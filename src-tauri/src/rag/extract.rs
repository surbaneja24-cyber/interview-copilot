//! Extraccion de texto plano a partir de los formatos que trae un candidato.
//!
//! Todo lo que entra aqui acaba siendo texto y nada mas: el indice no guarda formato,
//! tipografia ni imagenes. Lo unico que importa es que no se pierda contenido ni se
//! peguen palabras de columnas distintas.

use std::io::{Cursor, Read};
use std::path::Path;

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    PlainText,
    Markdown,
    Pdf,
    Docx,
}

impl Format {
    pub fn from_path(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        match extension.as_str() {
            "txt" => Some(Self::PlainText),
            "md" | "markdown" => Some(Self::Markdown),
            "pdf" => Some(Self::Pdf),
            "docx" => Some(Self::Docx),
            _ => None,
        }
    }
}

/// Lee un fichero del disco y devuelve su texto.
pub fn from_file(path: &Path) -> AppResult<String> {
    let format = Format::from_path(path).ok_or_else(|| {
        AppError::Invalid(format!(
            "Formato no soportado: {}. Se admiten .txt, .md, .pdf y .docx",
            path.display()
        ))
    })?;

    let bytes = std::fs::read(path)?;
    let text = from_bytes(&bytes, format)?;

    if text.trim().is_empty() {
        return Err(AppError::Invalid(format!(
            "\"{}\" no contiene texto extraible. Si es un PDF escaneado, hace falta OCR, \
             que todavia no esta implementado.",
            path.display()
        )));
    }

    let damage = orphan_letter_ratio(&text);
    if damage > MAX_ORPHAN_LETTER_RATIO {
        return Err(AppError::Invalid(format!(
            "El texto de \"{}\" sale roto: un {:.0}% de las palabras son letras sueltas. \
             El PDF dibuja cada letra por separado y el extractor no puede recomponerlas \
             sin inventarse donde van los espacios. Exporta el documento a .docx o .txt y \
             cargalo asi.",
            path.display(),
            damage * 100.0
        )));
    }

    Ok(text)
}

/// Por encima de esta proporcion de letras sueltas, el texto esta roto.
///
/// Referencia medida: un CV real extraido de un PDF con glifos posicionados uno a uno dio
/// 68 letras sueltas sobre 417 palabras (16%). Prosa en español normal se queda en 2-4%,
/// que son las conjunciones "y", "o", "a" y "e".
const MAX_ORPHAN_LETTER_RATIO: f32 = 0.08;

/// Proporcion de palabras que son una unica letra distinta de las conjunciones legitimas.
fn orphan_letter_ratio(text: &str) -> f32 {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() < 20 {
        return 0.0; // muy corto para juzgar
    }

    let orphans = words
        .iter()
        .filter(|word| {
            let mut chars = word.chars();
            match (chars.next(), chars.next()) {
                // Una sola letra que no sea conjuncion o preposicion de una letra.
                (Some(ch), None) => {
                    ch.is_alphabetic() && !matches!(ch.to_ascii_lowercase(), 'y' | 'o' | 'a' | 'e' | 'u')
                }
                _ => false,
            }
        })
        .count();

    orphans as f32 / words.len() as f32
}

pub fn from_bytes(bytes: &[u8], format: Format) -> AppResult<String> {
    let raw = match format {
        Format::PlainText | Format::Markdown => String::from_utf8_lossy(bytes).into_owned(),
        Format::Pdf => from_pdf(bytes)?,
        Format::Docx => from_docx(bytes)?,
    };

    Ok(clean(&raw))
}

/// Limpia y recompone el texto extraido antes de que lo vea el troceador.
///
/// Medido sobre un CV real de dos paginas extraido con `pdf-extract`: 2153 caracteres,
/// 6 bytes NUL, 128 saltos de linea y 83 lineas con **mediana de 20 caracteres**. Es
/// decir, el extractor devuelve una linea por linea visual del PDF y marca 45 supuestas
/// fronteras de parrafo. Trocear eso directamente corta el CV siguiendo la maquetacion en
/// vez del significado, y produce fragmentos que no terminan ni en un signo de puntuacion.
///
/// Los NUL, ademas, rompen cosas silenciosamente: `length()` de SQLite cuenta solo hasta
/// el primero, asi que un documento de 2153 caracteres se reportaba como de 45.
pub fn clean(text: &str) -> String {
    let sin_basura = strip_control_chars(text);
    let sin_guiones = join_hyphenated(&sin_basura);
    reflow(&sin_guiones)
}

/// Quita caracteres no imprimibles, conservando saltos y tabuladores.
fn strip_control_chars(text: &str) -> String {
    text.chars()
        .filter(|ch| match ch {
            '\n' | '\t' => true,
            // Marca de orden de bytes, guion blando y espacios de ancho cero: invisibles
            // y capaces de partir palabras dentro del tokenizador.
            '\u{FEFF}' | '\u{00AD}' | '\u{200B}' | '\u{200C}' | '\u{200D}' => false,
            _ => !ch.is_control(),
        })
        .collect()
}

/// Une las palabras que el PDF partio al final de linea: "desa-\nrrollo" → "desarrollo".
fn join_hyphenated(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '-' {
            // Mira si lo que sigue es un salto de linea y luego minuscula.
            let mut lookahead = chars.clone();
            let mut saw_newline = false;
            while let Some(next) = lookahead.peek() {
                match next {
                    '\n' => {
                        saw_newline = true;
                        lookahead.next();
                    }
                    ' ' | '\t' | '\r' => {
                        lookahead.next();
                    }
                    other if saw_newline && other.is_lowercase() => {
                        chars = lookahead;
                        break;
                    }
                    _ => break,
                }
            }
            if saw_newline && chars.peek().is_some_and(|c| c.is_lowercase()) {
                continue; // se traga el guion y el salto
            }
        }
        out.push(ch);
    }

    out
}

/// Recompone las lineas partidas por maquetacion.
///
/// La señal fiable no es la mayuscula inicial —"PHP a microservicios" empieza en
/// mayuscula y es continuacion de la linea anterior— sino **el ancho de linea**: si la
/// linea previa llega cerca del ancho maximo del documento, se partio porque no cabia
/// mas, no porque alli terminara una idea. Una linea corta seguida de otra es una
/// decision de quien escribio el CV y se respeta.
fn reflow(text: &str) -> String {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let full_width = detect_full_width(&normalized);

    let mut paragraphs: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut previous_was_wrapped = false;

    for raw_line in normalized.split('\n') {
        let line = raw_line.trim();

        if line.is_empty() {
            if !current.is_empty() {
                paragraphs.push(std::mem::take(&mut current));
            }
            previous_was_wrapped = false;
            continue;
        }

        let wrapped = line.chars().count() >= full_width;

        if current.is_empty() {
            current.push_str(line);
            previous_was_wrapped = wrapped;
            continue;
        }

        if continues_previous(&current, line, previous_was_wrapped) {
            current.push(' ');
            current.push_str(line);
        } else {
            paragraphs.push(std::mem::replace(&mut current, line.to_owned()));
        }

        previous_was_wrapped = wrapped;
    }

    if !current.is_empty() {
        paragraphs.push(current);
    }

    paragraphs.join("\n\n")
}

/// Ancho a partir del cual se considera que una linea se partio por no caber. Se calcula
/// del propio documento, porque depende de la fuente y los margenes de cada PDF.
fn detect_full_width(text: &str) -> usize {
    let mut widths: Vec<usize> = text
        .split('\n')
        .map(|line| line.trim().chars().count())
        .filter(|width| *width > 0)
        .collect();

    if widths.is_empty() {
        return usize::MAX;
    }

    widths.sort_unstable();
    // Percentil 90 como "linea llena", y se acepta como partida cualquiera que llegue al
    // 75% de eso: dentro de un parrafo justificado las lineas no miden todas igual.
    let index = (widths.len() * 9 / 10).min(widths.len() - 1);
    let p90 = widths[index];
    (p90 * 3) / 4
}

fn continues_previous(previous: &str, line: &str, previous_was_wrapped: bool) -> bool {
    let ends_closed = previous
        .trim_end()
        .chars()
        .last()
        .is_some_and(|ch| matches!(ch, '.' | '!' | '?' | ':' | ';'));

    // Una viñeta siempre abre unidad nueva, mida lo que mida la linea anterior.
    let starts_item = line.chars().next().is_some_and(is_bullet);

    !ends_closed && !starts_item && previous_was_wrapped
}

fn is_bullet(ch: char) -> bool {
    matches!(ch, '-' | '*' | '·' | '•' | '‣' | '▪' | '◦' | '–' | '—')
}

fn from_pdf(bytes: &[u8]) -> AppResult<String> {
    // pdf-extract entra en panic con algunos PDF malformados en vez de devolver error.
    // Un CV con una fuente rara no puede tumbar la aplicacion entera.
    let result = std::panic::catch_unwind(|| pdf_extract::extract_text_from_mem(bytes));

    match result {
        Ok(Ok(text)) => Ok(text),
        Ok(Err(err)) => Err(AppError::Invalid(format!("No se pudo leer el PDF: {err}"))),
        Err(_) => Err(AppError::Invalid(
            "No se pudo leer el PDF: el fichero parece estar corrupto o usa una \
             codificacion no soportada."
                .into(),
        )),
    }
}

/// Un .docx es un ZIP con `word/document.xml` dentro. Se leen los nodos `<w:t>` (texto) y
/// se corta parrafo en cada `</w:p>`; sin eso el documento entero saldria como una sola
/// linea y el troceado por parrafos perderia su mejor frontera.
fn from_docx(bytes: &[u8]) -> AppResult<String> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|err| AppError::Invalid(format!("No se pudo abrir el .docx: {err}")))?;

    let mut xml = String::new();
    archive
        .by_name("word/document.xml")
        .map_err(|_| {
            AppError::Invalid(
                "El .docx no contiene word/document.xml: puede ser un .doc antiguo \
                 renombrado."
                    .into(),
            )
        })?
        .read_to_string(&mut xml)?;

    Ok(text_from_document_xml(&xml))
}

fn text_from_document_xml(xml: &str) -> String {
    use quick_xml::events::Event;
    use quick_xml::Reader;

    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);

    let mut out = String::new();
    let mut inside_text = false;
    let mut buffer = Vec::new();

    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(tag)) => {
                if tag.local_name().as_ref() == b"t" {
                    inside_text = true;
                }
            }
            Ok(Event::End(tag)) => match tag.local_name().as_ref() {
                b"t" => inside_text = false,
                b"p" => out.push('\n'),
                _ => {}
            },
            Ok(Event::Empty(tag)) => {
                // Saltos de linea y tabuladores dentro de un parrafo.
                match tag.local_name().as_ref() {
                    b"br" => out.push('\n'),
                    b"tab" => out.push('\t'),
                    _ => {}
                }
            }
            Ok(Event::Text(text)) if inside_text => {
                out.push_str(&text.unescape().unwrap_or_default());
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buffer.clear();
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconoce_las_extensiones_soportadas() {
        assert_eq!(Format::from_path(Path::new("cv.pdf")), Some(Format::Pdf));
        assert_eq!(
            Format::from_path(Path::new("NOTAS.MD")),
            Some(Format::Markdown)
        );
        assert_eq!(
            Format::from_path(Path::new("carta.DOCX")),
            Some(Format::Docx)
        );
        assert_eq!(Format::from_path(Path::new("foto.png")), None);
        assert_eq!(Format::from_path(Path::new("sin_extension")), None);
    }

    #[test]
    fn texto_plano_pasa_tal_cual() {
        let text = from_bytes("Tres años en React.".as_bytes(), Format::PlainText)
            .expect("leer texto plano");
        assert_eq!(text, "Tres años en React.");
    }

    /// Los NUL rompen `length()` de SQLite en silencio: un CV de 2153 caracteres se
    /// reportaba como de 45 porque habia un NUL en esa posicion.
    #[test]
    fn elimina_caracteres_de_control() {
        let sucio = "Desarrollador\u{0}full-stack\u{FEFF} con\u{200B} experiencia.";
        let limpio = clean(sucio);
        assert!(!limpio.contains('\u{0}'));
        assert!(!limpio.contains('\u{FEFF}'));
        assert!(!limpio.contains('\u{200B}'));
        assert!(limpio.contains("experiencia"));
    }

    #[test]
    fn conserva_saltos_y_tabuladores() {
        let limpio = clean("Primera línea.\n\nSegunda línea.");
        assert!(limpio.contains("Primera línea."));
        assert!(limpio.contains("Segunda línea."));
    }

    /// El caso que motivo todo esto: el PDF parte cada frase por ancho de columna.
    #[test]
    fn recompone_frases_partidas_por_maquetacion() {
        let del_pdf = "Lideré la migración de un monolito\n\
                       PHP a microservicios en Node,\n\
                       coordinando a cuatro personas.";
        let limpio = clean(del_pdf);
        assert_eq!(
            limpio,
            "Lideré la migración de un monolito PHP a microservicios en Node, coordinando a cuatro personas."
        );
    }

    #[test]
    fn respeta_las_vinetas_como_unidades_separadas() {
        let del_pdf = "• Migración de monolito a microservicios\n\
                       • Clases particulares de matemáticas\n\
                       • Atención al cliente en cafetería";
        let limpio = clean(del_pdf);
        assert_eq!(
            limpio.matches("\n\n").count(),
            2,
            "cada viñeta deberia quedar como párrafo propio: {limpio}"
        );
    }

    #[test]
    fn no_pega_una_frase_nueva_a_la_anterior() {
        let del_pdf = "Reduje el despliegue a once minutos.\nDi clases de matemáticas.";
        let limpio = clean(del_pdf);
        assert!(
            limpio.contains("minutos.\n\nDi clases"),
            "dos frases cerradas no deben unirse: {limpio}"
        );
    }

    #[test]
    fn une_palabras_partidas_con_guion() {
        assert!(clean("desa-\nrrollo de software").contains("desarrollo de software"));
        // Un guion legitimo a final de frase no debe comerse nada.
        assert!(clean("full-stack\nDesarrollador").contains("full-stack"));
    }

    #[test]
    fn los_parrafos_de_docx_se_separan() {
        let xml = r#"<?xml version="1.0"?>
            <w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
              <w:body>
                <w:p><w:r><w:t>Primer párrafo.</w:t></w:r></w:p>
                <w:p><w:r><w:t>Segundo párrafo.</w:t></w:r></w:p>
              </w:body>
            </w:document>"#;

        let text = text_from_document_xml(xml);
        assert!(text.contains("Primer párrafo."));
        assert!(text.contains("Segundo párrafo."));
        assert!(
            text.contains("Primer párrafo.\n"),
            "los párrafos deben quedar separados, si no el troceado pierde su frontera"
        );
    }

    #[test]
    fn los_trozos_de_texto_partidos_se_reconstruyen() {
        // Word parte una frase en varios <w:t> cuando cambia el formato a media palabra.
        let xml = r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
            <w:p><w:r><w:t>Lideré la </w:t></w:r><w:r><w:t>migración</w:t></w:r><w:r><w:t> del monolito.</w:t></w:r></w:p>
        </w:document>"#;

        let text = text_from_document_xml(xml);
        assert!(text.contains("Lideré la migración del monolito."));
    }

    #[test]
    fn las_entidades_xml_se_desescapan() {
        let xml = r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
            <w:p><w:r><w:t>I+D &amp; calidad</w:t></w:r></w:p>
        </w:document>"#;

        assert!(text_from_document_xml(xml).contains("I+D & calidad"));
    }

    #[test]
    fn un_pdf_invalido_da_error_y_no_panic() {
        let result = from_bytes(b"esto no es un PDF", Format::Pdf);
        assert!(
            result.is_err(),
            "deberia devolver error, no entrar en panic"
        );
    }

    #[test]
    fn formato_no_soportado_se_rechaza_con_mensaje_util() {
        let dir = tempfile::tempdir().expect("directorio temporal");
        let path = dir.path().join("foto.png");
        std::fs::write(&path, b"x").expect("escribir");

        let error = from_file(&path).expect_err("deberia fallar");
        assert!(error.to_string().contains(".pdf"));
    }

    /// Con la forma real del dano medido: "Sta c k", "surba n e j a".
    #[test]
    fn detecta_el_texto_roto_por_glifos_sueltos() {
        let dir = tempfile::tempdir().expect("directorio temporal");
        let path = dir.path().join("cv.txt");
        std::fs::write(
            &path,
            "PRO YECTO S GESTO R DE INVENTARIO PO R VO Z Full Sta c k surba n e j a \
             desarrollo w e b con React n o d e y Express APIs",
        )
        .expect("escribir");

        let error = from_file(&path).expect_err("deberia rechazarse");
        assert!(
            error.to_string().contains(".docx"),
            "el mensaje debe decir que hacer: {error}"
        );
    }

    /// Prosa normal en español no puede dispararlo: "y", "o", "a" son palabras de verdad.
    #[test]
    fn la_prosa_normal_no_se_considera_rota() {
        let dir = tempfile::tempdir().expect("directorio temporal");
        let path = dir.path().join("cv.txt");
        std::fs::write(
            &path,
            "Lideré la migración de un monolito a microservicios y coordiné a cuatro \
             personas. Reduje el despliegue de dos horas a once minutos e introduje \
             pruebas de humo automáticas y la integración continua en el equipo.",
        )
        .expect("escribir");

        assert!(from_file(&path).is_ok(), "no deberia dar el texto por roto");
    }

    #[test]
    fn un_fichero_sin_texto_avisa_de_ocr() {
        let dir = tempfile::tempdir().expect("directorio temporal");
        let path = dir.path().join("vacio.txt");
        std::fs::write(&path, b"   \n  ").expect("escribir");

        let error = from_file(&path).expect_err("deberia fallar");
        assert!(error.to_string().contains("OCR"));
    }
}
