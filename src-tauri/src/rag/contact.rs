//! Datos de contacto fuera del indice (§31 del spec).
//!
//! La cabecera de un CV —telefono, correo, perfiles— se troceaba como un fragmento mas.
//! Nunca responde a una pregunta de entrevista y ocupa uno de los cinco huecos que se le
//! mandan al modelo, asi que el coste no es teorico: es un fragmento util menos en cada
//! pregunta. Ademas §31 pide no pasear datos personales por la pantalla, y un fragmento
//! recuperado se enseña junto a la respuesta.
//!
//! Se limpia **antes de trocear y por lineas**. En el texto que ya paso por
//! `chunking::normalize` los saltos simples son espacios, y para entonces el telefono y
//! la primera seccion del CV son el mismo parrafo: no hay forma de quitar uno sin
//! llevarse el otro.
//!
//! **Se quita el dato, no la linea.** La primera version tiraba la linea entera cuando
//! era solo contacto, y medida contra el CV real (2026-08-19) quito exactamente una linea
//! —el telefono— y dejo el correo dentro del indice: `pdf-extract` habia metido nombre,
//! puesto, correo y ciudad en una sola linea, asi que la linea tenia contenido de sobra y
//! se salvo con el correo dentro. La regla que si funciona sobre el documento real es
//! quitar el correo, el telefono y el perfil alla donde aparezcan, y tirar la linea solo
//! si lo que queda no llega a `MAX_RESIDUE_WORDS` palabras.
//!
//! Lo que esto **no** detecta, y conviene tenerlo escrito para no venderlo de mas:
//!
//! - **Un nombre suelto.** No hay regla mecanica que separe "SANTIAGO URBANEJA" de
//!   "EXPERIENCIA LABORAL": las dos son cortas, en mayusculas y sin punto final.
//!   Intentarlo se llevaria por delante los encabezados de seccion, que son la frontera
//!   semantica mas fuerte que tiene un CV (ver `chunking`).
//! - **Una direccion postal sin etiqueta.** "Calle Mayor 3, Igualada" es indistinguible
//!   de una linea cualquiera. Con etiqueta ("Direccion: …") si se va.
//!
//! Por eso esto no es un anonimizador y no debe describirse como tal: es un filtro que
//! saca del indice los datos de contacto que una maquina reconoce sin margen de duda.

/// Digitos minimos para que una secuencia de numeros cuente como telefono.
///
/// Nueve son los de un numero espanol. Por debajo de ahi las cifras de un CV son cifras
/// de un CV: "gestione un presupuesto de 600 000 euros" se queda en seis digitos y no
/// dispara nada, que es justo lo que tiene que pasar.
const PHONE_MIN_DIGITS: usize = 9;

/// Palabras que pueden quedar en la linea, ya sin los datos de contacto, para que la
/// linea se tire entera en vez de conservarse limpia.
///
/// Un nombre espanol con dos apellidos son tres palabras, mas la ciudad, cuatro: eso es
/// una cabecera y no hace falta en el indice. Por encima ya es prosa, y la prosa se
/// queda. Los dos fallos no cuestan lo mismo: conservar una cabecera gasta uno de los
/// cinco huecos, tirar una linea con contenido pierde experiencia del candidato sin que
/// nadie se entere. Por eso el numero es bajo y lo que se quita se cuenta y sube a la UI.
const MAX_RESIDUE_WORDS: usize = 4;

/// Etiquetas de contacto. Solo cuentan como senal cuando llevan dos puntos: "Telefono:"
/// encabeza un dato, "telefono de guardia" es una frase.
const LABELS: &[&str] = &[
    "telefono",
    "telefonos",
    "tel",
    "tlf",
    "movil",
    "celular",
    "email",
    "e-mail",
    "correo",
    "mail",
    "linkedin",
    "github",
    "direccion",
    "contacto",
    "phone",
    "mobile",
    "address",
];

/// Dominios cuyo perfil personal es un dato de contacto.
const PROFILE_HOSTS: &[&str] = &["linkedin.com", "github.com", "gitlab.com"];

pub struct Stripped {
    pub text: String,
    /// Cuantos datos de contacto se dejaron fuera: correos, telefonos y perfiles. Sube
    /// hasta la UI a proposito, porque es el unico dato con el que juzgar si la regla
    /// quita de mas o de menos, y juzgarlo a ojo es como se eligio mal el modelo de
    /// embeddings la primera vez.
    pub removed: usize,
}

pub fn strip(text: &str) -> Stripped {
    let mut lines: Vec<String> = Vec::new();
    let mut removed = 0usize;

    for line in text.lines() {
        let cleaned = clean_line(line);
        removed += cleaned.removed;

        // Una linea en blanco no tiene nada que quitar, asi que vuelve intacta. Importa:
        // perderla pegaria dos secciones del CV en el mismo parrafo, que es justo lo que
        // el troceado evita.
        if let Some(kept) = cleaned.kept {
            lines.push(kept);
        }
    }

    Stripped {
        text: lines.join("
"),
        removed,
    }
}

struct CleanLine {
    /// `None` si la linea no era mas que datos de contacto.
    kept: Option<String>,
    removed: usize,
}

/// Quita de una linea los datos de contacto, y decide si lo que queda merece indexarse.
fn clean_line(line: &str) -> CleanLine {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    let words: Vec<Word> = tokens.iter().copied().map(classify).collect();

    let mut kept: Vec<&str> = Vec::new();
    let mut removed = 0usize;
    let mut index = 0usize;

    while index < words.len() {
        match words[index] {
            // Secuencia de palabras numericas: se juzgan juntas o no hay telefono que
            // valga, porque suelto ninguno de los tres grupos llega a nueve digitos.
            Word::Digits(_) => {
                let start = index;
                let mut digits = 0usize;
                while let Some(Word::Digits(more)) = words.get(index) {
                    digits += more;
                    index += 1;
                }
                if digits >= PHONE_MIN_DIGITS {
                    removed += 1;
                } else {
                    kept.extend_from_slice(&tokens[start..index]);
                }
                continue;
            }
            Word::Contact => removed += 1,
            // El correo llego partido en dos palabras: la que falta es la de al lado.
            Word::EmailTail => {
                removed += 1;
                if kept.last().is_some_and(|previous| is_local_part(previous)) {
                    kept.pop();
                }
            }
            Word::EmailHead => {
                removed += 1;
                if tokens.get(index + 1).is_some_and(|next| is_domain(next)) {
                    index += 1;
                }
            }
            // Una etiqueta con dos puntos encabeza un dato de contacto: se va con el.
            // Sin dos puntos es una palabra normal ("telefono de guardia").
            Word::Label(true) => {}
            Word::Label(false) | Word::Other => kept.push(tokens[index]),
        }
        index += 1;
    }

    if removed == 0 {
        // Nada que quitar: la linea sale tal cual, con su sangria y sus espacios.
        return CleanLine {
            kept: Some(line.to_owned()),
            removed: 0,
        };
    }

    CleanLine {
        kept: (kept.len() > MAX_RESIDUE_WORDS).then(|| kept.join(" ")),
        removed,
    }
}

/// En que se convierte cada palabra de la linea al juzgarla.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Word {
    /// Palabra puramente numerica, con los digitos que trae. Un telefono llega partido en
    /// varias ("600 123 456"), asi que la decision no se puede tomar palabra a palabra.
    Digits(usize),
    Contact,
    /// La cola de un correo que el extractor de PDF partio por el espacio:
    /// "surbaneja24 @gmail.com". Se lleva por delante la palabra anterior, que es la
    /// parte local del correo.
    EmailTail,
    /// La cabeza del mismo destrozo al reves: "surbaneja24@ gmail.com".
    EmailHead,
    /// Etiqueta de contacto; `true` si trae dos puntos.
    Label(bool),
    Other,
}

fn classify(token: &str) -> Word {
    if let Some(digits) = numeric_token(token) {
        return Word::Digits(digits);
    }
    if is_email(token) || is_profile_url(token) {
        return Word::Contact;
    }
    if let Some(domain) = trim_separators(token).strip_prefix('@') {
        if is_domain(domain) {
            return Word::EmailTail;
        }
    }
    if trim_separators(token).ends_with('@') {
        return Word::EmailHead;
    }
    match label(token) {
        Some(titled) => Word::Label(titled),
        None => Word::Other,
    }
}

/// Digitos del token si es puramente numerico (admite el formato de un telefono:
/// signos mas, guiones, puntos y parentesis). `None` si lleva letras.
fn numeric_token(token: &str) -> Option<usize> {
    let mut digits = 0usize;
    for ch in token.chars() {
        if ch.is_ascii_digit() {
            digits += 1;
        } else if !matches!(ch, '+' | '-' | '.' | '(' | ')' | '/' | '·' | '|') {
            return None;
        }
    }
    (digits > 0).then_some(digits)
}

fn is_email(token: &str) -> bool {
    let token = trim_separators(token);
    let Some((local, domain)) = token.split_once('@') else {
        return false;
    };
    !local.is_empty() && !local.contains('@') && is_domain(domain)
}

fn is_domain(text: &str) -> bool {
    trim_separators(text)
        .rsplit_once('.')
        .is_some_and(|(host, tld)| {
            !host.is_empty() && tld.len() >= 2 && tld.chars().all(char::is_alphabetic)
        })
}

/// Lo que puede ser la parte de la izquierda de un correo. Se exige que no tenga puntos
/// finales ni signos raros para no llevarse por delante la palabra anterior de una frase.
fn is_local_part(token: &str) -> bool {
    !token.is_empty()
        && token
            .chars()
            .all(|ch| ch.is_alphanumeric() || matches!(ch, '.' | '_' | '-' | '+' | '%'))
}

/// Perfil personal, no cualquier enlace.
///
/// La distincion importa: `github.com/santiago` es un dato de contacto y
/// `github.com/santiago/interview-copilot` es un proyecto del candidato. Tratar los dos
/// igual borraria la linea que nombra el proyecto, que es justo lo que hay que indexar.
fn is_profile_url(token: &str) -> bool {
    let lowered = trim_separators(token).to_lowercase();
    let without_scheme = lowered
        .strip_prefix("https://")
        .or_else(|| lowered.strip_prefix("http://"))
        .unwrap_or(&lowered);
    let bare = without_scheme.strip_prefix("www.").unwrap_or(without_scheme);

    let (host, path) = bare.split_once('/').unwrap_or((bare, ""));
    if !PROFILE_HOSTS.contains(&host) {
        return false;
    }

    let segments: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    match segments.as_slice() {
        // linkedin.com o github.com a secas: solo puede ser "mi perfil esta ahi".
        [] => true,
        // linkedin.com/in/santiago
        ["in", _] => true,
        // github.com/santiago, pero no github.com/santiago/proyecto
        [_] => true,
        _ => false,
    }
}

/// `Some(true)` si es una etiqueta de contacto con dos puntos, `Some(false)` si es la
/// misma palabra suelta, `None` si no es una etiqueta.
fn label(token: &str) -> Option<bool> {
    let has_colon = token.ends_with(':');
    let word = fold(trim_separators(token));
    LABELS.contains(&word.as_str()).then_some(has_colon)
}

/// Quita los separadores que adornan una cabecera ("·", "|", comas) y los dos puntos.
fn trim_separators(token: &str) -> &str {
    token.trim_matches(|ch: char| matches!(ch, '·' | '|' | ',' | ';' | ':' | '(' | ')' | '"'))
}

/// Minusculas sin tildes, para que "Teléfono" y "Telefono" sean la misma etiqueta.
fn fold(text: &str) -> String {
    text.chars()
        .flat_map(char::to_lowercase)
        .map(|ch| match ch {
            'á' | 'à' | 'ä' | 'â' => 'a',
            'é' | 'è' | 'ë' | 'ê' => 'e',
            'í' | 'ì' | 'ï' | 'î' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' => 'o',
            'ú' | 'ù' | 'ü' | 'û' => 'u',
            other => other,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Que es lo que se queda del texto, para no repetir el mismo `strip(...).text`.
    fn limpio(text: &str) -> String {
        strip(text).text
    }

    #[test]
    fn la_cabecera_de_un_cv_se_va_entera() {
        let cv = "SANTIAGO URBANEJA
                  Teléfono: 600 123 456
                  correo@ejemplo.com
                  linkedin.com/in/santiago-urbaneja

                  EXPERIENCIA
                  Lideré la migración de un monolito.";

        let stripped = strip(cv);
        assert_eq!(stripped.removed, 3);
        assert!(!stripped.text.contains("600"));
        assert!(!stripped.text.contains('@'));
        assert!(!stripped.text.contains("linkedin"));
        assert!(stripped.text.contains("Lideré la migración"));
    }

    /// El caso que obligo a cambiar la regla, medido contra el CV real el 2026-08-19:
    /// `pdf-extract` devuelve nombre, puesto, correo y ciudad en la misma linea. Con la
    /// regla anterior —tirar la linea entera o nada— la linea tenia contenido de sobra,
    /// se salvaba, y el correo acababa indexado.
    #[test]
    fn un_correo_en_medio_de_la_cabecera_se_va_y_el_resto_se_queda() {
        let cabecera = "Santiago Urbaneja Profesional de logística y almacén                         correo@ejemplo.com Igualada, España Perfil Profesional";

        let stripped = strip(cabecera);
        assert_eq!(stripped.removed, 1);
        assert!(!stripped.text.contains('@'), "{}", stripped.text);
        assert!(stripped.text.contains("Profesional de logística y almacén"));
        assert!(stripped.text.contains("Igualada"));
    }

    /// Segundo hallazgo de la medicion del 2026-08-19: `pdf-extract` parte el correo por
    /// el espacio y deja "surbaneja24 @gmail.com". Mirando token a token, ni la primera
    /// mitad ni la segunda son un correo, y el correo entero se colaba al indice.
    #[test]
    fn un_correo_partido_por_el_extractor_tambien_se_va() {
        let cabecera = "Santiago Urbaneja Profesional de logística y almacén                         surbaneja24 @gmail.com Igualada, España Perfil Profesional";

        let stripped = strip(cabecera);
        assert_eq!(stripped.removed, 1);
        assert!(!stripped.text.contains('@'), "{}", stripped.text);
        assert!(!stripped.text.contains("surbaneja24"), "{}", stripped.text);
        assert!(stripped.text.contains("Profesional de logística y almacén"));
    }

    /// El destrozo simetrico: la arroba se queda pegada a la parte local.
    #[test]
    fn un_correo_partido_por_el_otro_lado_tambien() {
        let stripped = strip("Contacto surbaneja24@ gmail.com");
        assert_eq!(stripped.removed, 1);
        assert!(!stripped.text.contains("gmail"), "{}", stripped.text);
    }

    /// Un dominio suelto en una frase no es un correo partido: no hay arroba por medio.
    #[test]
    fn un_dominio_en_una_frase_no_se_toca() {
        let frase = "Desplegué la tienda en midominio.com durante el segundo trimestre.";
        assert_eq!(limpio(frase), frase);
    }

    /// Una frase de verdad no se pierde por llevar un correo dentro: se queda sin el.
    #[test]
    fn una_frase_con_un_correo_dentro_conserva_la_frase() {
        let linea = "Automaticé el envío de avisos a soporte@empresa.com y reduje el tiempo                      de respuesta a la mitad.";
        let stripped = strip(linea);
        assert_eq!(stripped.removed, 1);
        assert!(stripped.text.starts_with("Automaticé el envío de avisos a y reduje"));
    }

    /// El caso que obliga a exigir nueve digitos: un CV esta lleno de cifras.
    #[test]
    fn una_cifra_grande_no_es_un_telefono() {
        let presupuesto = "Gestioné un presupuesto de 600 000 euros anuales";
        assert_eq!(limpio(presupuesto), presupuesto);

        let despliegue = "Reduje el despliegue de 120 minutos a 11";
        assert_eq!(limpio(despliegue), despliegue);
    }

    /// La razon de mirar cuantos segmentos tiene la ruta: el perfil es contacto, el
    /// repositorio es un proyecto del candidato y hay que indexarlo.
    #[test]
    fn un_repositorio_no_es_un_perfil() {
        assert_eq!(limpio("github.com/santiago"), "");
        assert_eq!(limpio("https://www.linkedin.com/in/santiago-urbaneja"), "");

        let proyecto = "Copiloto de entrevistas — github.com/santiago/interview-copilot";
        assert_eq!(limpio(proyecto), proyecto);
    }

    #[test]
    fn una_etiqueta_sin_dos_puntos_no_es_una_etiqueta() {
        let guardia = "Teléfono de guardia los fines de semana del mes";
        assert_eq!(limpio(guardia), guardia);
        assert_eq!(limpio("Teléfono: 600123456"), "");
    }

    /// Los saltos de parrafo son la frontera semantica del troceado: limpiar no puede
    /// pegar dos secciones.
    #[test]
    fn los_saltos_de_parrafo_sobreviven() {
        let texto = "EXPERIENCIA
Algo largo de verdad.

correo@ejemplo.com

FORMACIÓN
Otra cosa.";
        let stripped = strip(texto);
        assert_eq!(stripped.removed, 1);
        assert!(stripped.text.contains("Algo largo de verdad.

"));
        assert!(stripped.text.contains("

FORMACIÓN"));
    }

    /// Documenta el limite, no lo celebra: un nombre suelto se indexa igual. Si algun dia
    /// se detecta, este test es el que hay que cambiar a proposito.
    #[test]
    fn un_nombre_suelto_no_se_detecta() {
        assert_eq!(limpio("SANTIAGO URBANEJA"), "SANTIAGO URBANEJA");
        assert_eq!(limpio("Igualada, España"), "Igualada, España");
    }

    #[test]
    fn un_texto_sin_datos_de_contacto_sale_igual() {
        let texto = "EXPERIENCIA

Lideré la migración de un monolito PHP.";
        let stripped = strip(texto);
        assert_eq!(stripped.removed, 0);
        assert_eq!(stripped.text, texto);
    }

    /// Medicion contra un CV de verdad, que es lo unico que dice si `MAX_RESIDUE_WORDS`
    /// esta bien puesto. Imprime las lineas que se tiran con los digitos y los correos
    /// enmascarados: calibrar no exige ver el telefono de nadie.
    ///
    /// `INTERVIEW_COPILOT_CV=<ruta> cargo test --lib -- --ignored --nocapture mide_contra_un_cv_real`
    #[test]
    #[ignore = "necesita un CV real, que no vive en el repositorio"]
    fn mide_contra_un_cv_real() {
        let Ok(path) = std::env::var("INTERVIEW_COPILOT_CV") else {
            panic!("define INTERVIEW_COPILOT_CV con la ruta de un CV");
        };

        let content = crate::rag::extract::from_file(std::path::Path::new(&path))
            .expect("extraer el texto del CV");

        let mut removed = Vec::new();
        for line in content.lines() {
            let cleaned = clean_line(line);
            if cleaned.removed > 0 {
                removed.push((mask(line), cleaned.kept.map(|kept| mask(&kept))));
            }
        }

        let stripped = strip(&content);
        let antes = crate::rag::chunking::split(&content);
        let despues = crate::rag::chunking::split(&stripped.text);

        println!("lineas tocadas: {}", removed.len());
        for (before, after) in &removed {
            println!("  - {before}");
            match after {
                Some(kept) => println!("    queda: {kept}"),
                None => println!("    queda: (nada, la linea se va)"),
            }
        }
        println!("fragmentos antes: {}, despues: {}", antes.len(), despues.len());
        for chunk in &despues {
            println!("  [{}] {}", chunk.text.chars().count(), mask(&chunk.text));
        }
    }

    fn mask(text: &str) -> String {
        let mut out: Vec<String> = Vec::new();
        for token in text.split_whitespace() {
            // Un correo partido por el extractor deja la parte local en la palabra
            // anterior: enmascarar solo la que lleva la arroba no serviria de nada.
            if token.starts_with('@') && !out.is_empty() {
                out.pop();
            }
            if token.contains('@') {
                out.push("<correo>".to_owned());
                continue;
            }
            out.push(
                token
                    .chars()
                    .map(|ch| if ch.is_ascii_digit() { '#' } else { ch })
                    .collect(),
            );
        }
        out.join(" ")
    }
}
