//! Troceado de documentos para indexar.
//!
//! El tamano del trozo decide la calidad de la recuperacion mas que casi ninguna otra
//! cosa del sistema. Trozos muy grandes recuperan ruido: la pregunta acierta el trozo
//! pero el LLM recibe tres experiencias mezcladas y responde generico. Trozos muy
//! pequenos pierden el contexto que los hace utiles ("coordinando a cuatro personas"
//! sin saber de que proyecto habla).
//!
//! Estrategia: cortar por fronteras semanticas de mayor a menor —parrafos, luego
//! frases— y solo partir a lo bruto cuando una frase sola ya excede el maximo. Cada
//! trozo se solapa con el anterior para que una idea a caballo entre dos no se pierda.

use serde::Serialize;

/// Tamano objetivo en caracteres. `multilingual-e5-small` acepta 512 tokens, que en
/// espanol son unos 2000 caracteres; se queda muy por debajo a proposito, porque el
/// limite util para recuperar con precision es bastante menor que el limite tecnico.
pub const TARGET_CHARS: usize = 700;

/// Nunca se emite un trozo mayor que esto, ni partiendo frases.
pub const MAX_CHARS: usize = 1000;

/// Solape entre trozos consecutivos.
pub const OVERLAP_CHARS: usize = 120;

/// Por debajo de esto un trozo no aporta nada recuperable y se fusiona con el vecino.
const MIN_CHARS: usize = 80;

/// Limite de una unidad indivisible antes de partirla.
///
/// No es `MAX_CHARS` a proposito: a un trozo que arranca con solape se le anaden hasta
/// `OVERLAP_CHARS` por delante, asi que si la unidad midiera ya el maximo el trozo final
/// se pasaria. Reservar el hueco del solape aqui es lo que garantiza el limite duro.
const UNIT_MAX_CHARS: usize = MAX_CHARS - OVERLAP_CHARS;

/// Hasta este tamano, un trozo puede seguir absorbiendo el parrafo siguiente; a partir de
/// aqui, un parrafo nuevo abre trozo nuevo.
///
/// Es el equilibrio entre dos fallos opuestos. Sin limite, dos secciones sustanciales de
/// un CV acaban en el mismo trozo y ninguna se recupera limpia. Con limite cero, una lista
/// de viñetas produce veinte trozos de una linea, cada uno sin contexto suficiente para
/// significar nada.
const MERGE_ACROSS_PARAGRAPHS_BELOW: usize = 300;

/// Longitud maxima de lo que puede pasar por encabezado de seccion.
const HEADING_MAX_CHARS: usize = 60;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Chunk {
    pub text: String,
    /// Posicion en caracteres dentro del documento original. Permite citar la fuente y
    /// resaltar de donde salio una respuesta (§5 de la arquitectura).
    pub start: usize,
    pub end: usize,
}

pub fn split(text: &str) -> Vec<Chunk> {
    let normalized = normalize(text);
    if normalized.trim().is_empty() {
        return Vec::new();
    }

    let units = segment(&normalized);
    let chunks = assemble(&normalized, &units);
    merge_runts(chunks)
}

/// Unifica saltos de linea y colapsa espacios sin destruir los cortes de parrafo, que
/// son la frontera semantica mas fiable de un CV.
fn normalize(text: &str) -> String {
    let unified = text.replace("\r\n", "\n").replace('\r', "\n");

    let mut out = String::with_capacity(unified.len());
    let mut newlines = 0usize;

    for ch in unified.chars() {
        if ch == '\n' {
            newlines += 1;
            continue;
        }
        if newlines > 0 {
            out.push_str(if newlines >= 2 { "\n\n" } else { " " });
            newlines = 0;
        }
        if ch == ' ' || ch == '\t' {
            if !out.ends_with(' ') && !out.ends_with('\n') {
                out.push(' ');
            }
            continue;
        }
        out.push(ch);
    }

    out.trim().to_owned()
}

/// Unidad indivisible de texto, con la marca de si abre parrafo.
#[derive(Debug, Clone, Copy)]
struct Unit {
    start: usize,
    end: usize,
    /// Verdadero si esta unidad empieza un parrafo nuevo. Decide si se le pone solape por
    /// delante: ver `assemble`.
    opens_paragraph: bool,
    /// Verdadero si la unidad parece un encabezado de seccion ("EXPERIENCIA", "FORMACIÓN").
    is_heading: bool,
}

/// Un encabezado de seccion de CV: linea corta, mayoritariamente en mayusculas y sin
/// terminar en punto.
///
/// Detectarlos importa porque son la frontera semantica mas fuerte de un curriculum, mas
/// que el salto de parrafo. Sin esto, el troceador pega el final de un empleo con el
/// titulo del siguiente, o el nombre y el telefono del candidato con la primera seccion,
/// y produce fragmentos que mezclan cosas sin relacion.
fn looks_like_heading(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.chars().count() > HEADING_MAX_CHARS {
        return false;
    }

    let letters = trimmed.chars().filter(|ch| ch.is_alphabetic());
    let (total, upper) = letters.fold((0usize, 0usize), |(total, upper), ch| {
        (total + 1, upper + usize::from(ch.is_uppercase()))
    });

    if total < 3 {
        return false;
    }

    let mostly_upper = upper * 10 >= total * 7;
    let ends_open = !trimmed.ends_with(['.', '!', '?']);

    mostly_upper && ends_open
}

/// Corta el texto en unidades indivisibles: parrafos si caben, si no frases, y si una
/// frase sola pasa del maximo, trozos por limite de palabra.
fn segment(text: &str) -> Vec<Unit> {
    let mut units = Vec::new();

    for (para_start, paragraph) in paragraphs(text) {
        if paragraph.chars().count() <= UNIT_MAX_CHARS {
            units.push(Unit {
                start: para_start,
                end: para_start + paragraph.len(),
                opens_paragraph: true,
                is_heading: looks_like_heading(paragraph),
            });
            continue;
        }

        let mut first_of_paragraph = true;
        for (sent_start, sentence) in sentences(paragraph, para_start) {
            if sentence.chars().count() <= UNIT_MAX_CHARS {
                units.push(Unit {
                    start: sent_start,
                    end: sent_start + sentence.len(),
                    opens_paragraph: first_of_paragraph,
                    is_heading: false,
                });
            } else {
                for (index, (start, end)) in hard_split(sentence, sent_start).into_iter().enumerate()
                {
                    units.push(Unit {
                        start,
                        end,
                        opens_paragraph: first_of_paragraph && index == 0,
                        is_heading: false,
                    });
                }
            }
            first_of_paragraph = false;
        }
    }

    units
}

fn paragraphs(text: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut offset = 0usize;

    for part in text.split("\n\n") {
        let trimmed = part.trim();
        if !trimmed.is_empty() {
            // El offset del texto recortado dentro del original.
            let lead = part.len() - part.trim_start().len();
            out.push((offset + lead, trimmed));
        }
        offset += part.len() + 2;
    }

    out
}

/// Corte de frases suficiente para prosa de CV y ofertas de empleo. No intenta manejar
/// abreviaturas ("Dr.", "S.L.") porque partir de mas es inofensivo aqui: el solape
/// recupera el contexto perdido.
fn sentences(text: &str, base: usize) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut start = 0usize;

    let bytes: Vec<(usize, char)> = text.char_indices().collect();
    for (index, (byte_pos, ch)) in bytes.iter().enumerate() {
        if !matches!(ch, '.' | '!' | '?' | ';' | '\n') {
            continue;
        }
        let next_is_space = bytes
            .get(index + 1)
            .is_none_or(|(_, next)| next.is_whitespace());
        if !next_is_space {
            continue;
        }

        let end = byte_pos + ch.len_utf8();
        let slice = text[start..end].trim();
        if !slice.is_empty() {
            let lead = text[start..end].len() - text[start..end].trim_start().len();
            out.push((base + start + lead, slice));
        }
        start = end;
    }

    let tail = text[start..].trim();
    if !tail.is_empty() {
        let lead = text[start..].len() - text[start..].trim_start().len();
        out.push((base + start + lead, tail));
    }

    out
}

/// Ultimo recurso: partir por limite de palabra sin pasarse de `MAX_CHARS`.
fn hard_split(text: &str, base: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = 0usize;

    while start < text.len() {
        let remaining = &text[start..];
        if remaining.chars().count() <= UNIT_MAX_CHARS {
            out.push((base + start, base + text.len()));
            break;
        }

        let limit = byte_index_at_char(remaining, UNIT_MAX_CHARS);
        let cut = remaining[..limit]
            .rfind(' ')
            .map_or(limit, |space| space + 1);

        out.push((base + start, base + start + cut));
        start += cut;
    }

    out
}

fn byte_index_at_char(text: &str, chars: usize) -> usize {
    text.char_indices()
        .nth(chars)
        .map_or(text.len(), |(index, _)| index)
}

/// Junta unidades consecutivas hasta acercarse a `TARGET_CHARS`, y arranca la siguiente
/// con solape.
///
/// **El solape no cruza fronteras de parrafo.** Esto se aprendio midiendo: con solape
/// indiscriminado, un CV de tres secciones producia trozos que eran "el final de lideré
/// una migración" + "el principio de di clases de matemáticas". Cada trozo mezclaba dos
/// experiencias sin relacion y ninguna de las dos se recuperaba limpia. El solape existe
/// para no partir una idea continua a la mitad; entre dos secciones distintas de un CV no
/// hay ninguna idea que proteger, solo ruido que anadir.
fn assemble(text: &str, units: &[Unit]) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    let mut current: Option<(usize, usize)> = None;
    // Verdadero cuando el trozo en construccion ya contiene texto de mas de un parrafo.
    let mut spans_paragraphs = false;
    // Un trozo no puede cerrarse si solo contiene encabezados: no responderia a nada.
    let mut current_has_body = false;

    for unit in units.iter().copied() {
        match current {
            None => {
                current = Some((unit.start, unit.end));
                spans_paragraphs = false;
                current_has_body = !unit.is_heading;
            }
            Some((cur_start, cur_end)) => {
                let would_be = text[cur_start..unit.end].chars().count();

                // Un trozo que cruza parrafos solo se tolera mientras siga siendo corto:
                // son viñetas sueltas que por separado no dirian nada. En cuanto hay
                // contenido sustancial, cada parrafo va a su trozo.
                let cap = if spans_paragraphs || unit.opens_paragraph {
                    MERGE_ACROSS_PARAGRAPHS_BELOW
                } else {
                    TARGET_CHARS
                };

                // Un encabezado de seccion abre trozo, porque es la frontera mas fuerte
                // que hay en un CV. Pero solo cierra el anterior si ese anterior tiene
                // contenido de verdad: dos encabezados seguidos ("PROYECTOS" y debajo el
                // titulo del proyecto, ambos en mayusculas) dejarian el primero como un
                // fragmento de una palabra, que no responde a ninguna pregunta.
                if unit.is_heading {
                    if current_has_body {
                        chunks.push(emit(text, cur_start, cur_end));
                        spans_paragraphs = false;
                        current = Some((unit.start, unit.end));
                    } else {
                        // Encabezados consecutivos se acumulan a la espera de contenido.
                        current = Some((cur_start, unit.end));
                    }
                    continue;
                }
                current_has_body = true;

                if would_be <= cap {
                    spans_paragraphs = spans_paragraphs || unit.opens_paragraph;
                    current = Some((cur_start, unit.end));
                } else {
                    spans_paragraphs = false;
                    current_has_body = true;
                    chunks.push(emit(text, cur_start, cur_end));
                    let next_start = if unit.opens_paragraph {
                        unit.start
                    } else {
                        back_off(text, cur_end, unit.start)
                    };
                    current = Some((next_start, unit.end));
                }
            }
        }
    }

    if let Some((start, end)) = current {
        chunks.push(emit(text, start, end));
    }

    chunks
}

/// Retrocede desde el final del trozo anterior para crear el solape, sin cortar palabras
/// ni retroceder mas alla del principio de la unidad nueva.
fn back_off(text: &str, previous_end: usize, next_start: usize) -> usize {
    let window_start = byte_index_back(text, previous_end, OVERLAP_CHARS);
    if window_start >= next_start {
        return next_start;
    }

    text[window_start..next_start]
        .find(' ')
        .map_or(next_start, |space| window_start + space + 1)
}

fn byte_index_back(text: &str, from: usize, chars: usize) -> usize {
    text[..from]
        .char_indices()
        .rev()
        .nth(chars.saturating_sub(1))
        .map_or(0, |(index, _)| index)
}

fn emit(text: &str, start: usize, end: usize) -> Chunk {
    Chunk {
        text: text[start..end].trim().to_owned(),
        start,
        end,
    }
}

/// Un trozo final diminuto no se recupera nunca por si solo; se pega al anterior.
fn merge_runts(mut chunks: Vec<Chunk>) -> Vec<Chunk> {
    if chunks.len() < 2 {
        return chunks;
    }

    let last_is_runt = chunks
        .last()
        .is_some_and(|chunk| chunk.text.chars().count() < MIN_CHARS);

    if last_is_runt {
        let runt = chunks.pop().expect("acabamos de comprobar que hay dos");
        if let Some(previous) = chunks.last_mut() {
            if runt.end.saturating_sub(previous.start) <= MAX_CHARS {
                previous.text.push(' ');
                previous.text.push_str(&runt.text);
                previous.end = runt.end;
            } else {
                chunks.push(runt);
            }
        }
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cv() -> String {
        [
            "Santiago Urbaneja — Desarrollador Full-Stack.",
            "",
            "Lideré la migración de un monolito PHP a microservicios en Node durante seis meses. \
             Coordiné a cuatro personas y reduje el tiempo de despliegue de dos horas a once minutos. \
             El mayor obstáculo fue la base de datos compartida, que resolvimos con un patrón de \
             strangler fig y doble escritura durante la transición.",
            "",
            "Bootcamp full-stack en 4Geeks. Formación en React, Node y bases de datos relacionales.",
        ]
        .join("\n")
    }

    #[test]
    fn texto_vacio_no_produce_trozos() {
        assert!(split("").is_empty());
        assert!(split("   \n\n  \t ").is_empty());
    }

    #[test]
    fn un_texto_corto_es_un_solo_trozo() {
        let chunks = split("Tres años de experiencia en React.");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text, "Tres años de experiencia en React.");
    }

    #[test]
    fn ningun_trozo_pasa_del_maximo() {
        let long = "Frase de relleno para estirar el documento. ".repeat(200);
        for chunk in split(&long) {
            assert!(
                chunk.text.chars().count() <= MAX_CHARS,
                "trozo de {} caracteres",
                chunk.text.chars().count()
            );
        }
    }

    #[test]
    fn los_offsets_apuntan_al_texto_real() {
        let chunks = split(&cv());
        let normalized = normalize(&cv());

        for chunk in &chunks {
            assert!(chunk.end <= normalized.len());
            assert!(chunk.start < chunk.end);
            // El texto del trozo tiene que estar contenido en el rango que declara.
            assert!(normalized[chunk.start..chunk.end].contains(chunk.text.trim()));
        }
    }

    #[test]
    fn no_se_pierde_contenido_entre_trozos() {
        let chunks = split(&cv());
        let unido: String = chunks
            .iter()
            .map(|chunk| chunk.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");

        for fragmento in ["strangler fig", "cuatro personas", "once minutos", "4Geeks"] {
            assert!(unido.contains(fragmento), "se perdio: {fragmento}");
        }
    }

    #[test]
    fn los_acentos_no_parten_caracteres() {
        let texto = "Añadí gestión de configuración. ".repeat(60);
        for chunk in split(&texto) {
            // Si un corte cayera en mitad de un caracter multibyte, esto entraria en
            // panic al construir el String.
            assert!(chunk.text.is_char_boundary(0));
            assert!(!chunk.text.contains('\u{FFFD}'));
        }
    }

    #[test]
    fn una_frase_gigante_se_parte_por_palabras() {
        let sin_puntos = "palabra ".repeat(400);
        let chunks = split(&sin_puntos);
        assert!(chunks.len() > 1);
        for chunk in &chunks {
            assert!(chunk.text.chars().count() <= MAX_CHARS);
            assert!(
                !chunk.text.starts_with("abra"),
                "cortó una palabra por la mitad"
            );
        }
    }

    #[test]
    fn hay_solape_dentro_de_un_texto_continuo() {
        let long = "Gestioné el despliegue continuo del equipo de plataforma. ".repeat(40);
        let chunks = split(&long);
        assert!(chunks.len() > 1, "el texto deberia dar para varios trozos");

        let solapa = chunks.windows(2).any(|pair| pair[1].start < pair[0].end);
        assert!(solapa, "ningun trozo solapa con el anterior");
    }

    #[test]
    fn reconoce_los_encabezados_de_seccion() {
        assert!(looks_like_heading("EXPERIENCIA"));
        assert!(looks_like_heading("FORMACIÓN ACADÉMICA"));
        assert!(looks_like_heading("SANTIAGO URBANEJA"));
        // No son encabezados:
        assert!(!looks_like_heading("Lideré la migración del monolito PHP."));
        assert!(!looks_like_heading("de"));
        assert!(!looks_like_heading(
            "UNA LÍNEA EN MAYÚSCULAS TAN LARGA QUE YA NO PUEDE SER UN ENCABEZADO DE SECCIÓN SINO TEXTO"
        ));
    }

    /// El caso real del CV de prueba: sin esto, el nombre, el teléfono y el primer
    /// encabezado acababan en el mismo fragmento que el inicio de la experiencia.
    #[test]
    fn un_encabezado_abre_fragmento_y_no_lo_cierra() {
        let cv = [
            "SANTIAGO URBANEJA",
            "Desarrollador full-stack",
            "+34 600 000 000 · correo@ejemplo.com",
            "EXPERIENCIA",
            "Lideré la migración de un monolito PHP a microservicios en Node durante seis \
             meses, coordinando a cuatro personas del equipo de plataforma y reduciendo el \
             despliegue de dos horas a once minutos.",
            "FORMACIÓN",
            "Bootcamp full-stack en 4Geeks, con proyecto final de un SaaS de inventario.",
        ]
        .join("\n\n");

        let chunks = split(&cv);

        let con_experiencia = chunks
            .iter()
            .find(|chunk| chunk.text.contains("EXPERIENCIA"))
            .expect("deberia existir un fragmento que empiece por EXPERIENCIA");
        assert!(
            con_experiencia.text.trim_start().starts_with("EXPERIENCIA"),
            "el encabezado debe abrir el fragmento: {}",
            con_experiencia.text
        );
        assert!(
            con_experiencia.text.contains("monolito"),
            "el encabezado debe ir pegado a lo que encabeza"
        );
        assert!(
            !con_experiencia.text.contains("600 000 000"),
            "los datos de contacto no deben caer en el fragmento de experiencia"
        );
        assert!(
            !con_experiencia.text.contains("FORMACIÓN"),
            "dos secciones distintas no pueden compartir fragmento"
        );
    }

    /// Lo que rompio el test de extremo a extremo: con solape indiscriminado, cada trozo
    /// acababa siendo el final de una experiencia pegado al principio de otra.
    #[test]
    fn el_solape_no_cruza_fronteras_de_parrafo() {
        // Con puntuacion real: un CV tiene frases, y el troceado usa las frases como
        // frontera de segundo nivel.
        let seccion =
            |titulo: &str, relleno: &str| format!("{titulo}. {}", format!("{relleno}. ").repeat(8));
        let cv = [
            seccion("EXPERIENCIA", "Lideré la migración del monolito con strangler fig"),
            seccion("DOCENCIA", "Di clases particulares de matemáticas a bachillerato"),
            seccion("CAFETERÍA", "Llevé la caja y la atención al cliente en horas punta"),
        ]
        .join("\n\n");

        let chunks = split(&cv);
        assert!(chunks.len() >= 3, "deberian salir varios trozos");

        for chunk in &chunks {
            let temas = ["strangler fig", "clases particulares", "atención al cliente"]
                .iter()
                .filter(|tema| chunk.text.contains(*tema))
                .count();
            assert!(
                temas <= 1,
                "un trozo mezcla {temas} experiencias distintas: {}",
                &chunk.text[..chunk.text.len().min(120)]
            );
        }
    }
}
