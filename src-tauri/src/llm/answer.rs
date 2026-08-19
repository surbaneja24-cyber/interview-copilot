//! La respuesta estructurada de §8 y su lectura.
//!
//! Dos cosas viven aqui y conviene entender por que estan juntas:
//!
//! 1. **El parseo tolerante.** Un modelo devuelve JSON *casi* siempre. Devuelve tambien
//!    vallas de codigo, una frase de cortesia antes del objeto, `keyPoints` en vez de
//!    `key_points`, o una cadena donde se pidio una lista. `response_format` ayuda pero
//!    no todos los servidores lo aceptan, asi que el parseo no puede depender de el.
//!    Cada tolerancia de este modulo esta ahi porque es un fallo real de modelos reales,
//!    y cada una tiene su test.
//!
//! 2. **El extractor incremental.** La UI tiene que ensenar la respuesta segun se
//!    escribe (§10), pero la respuesta es un objeto JSON y un JSON a medias no se puede
//!    parsear. `StreamScanner` va sacando el valor del campo `answer` caracter a caracter
//!    mientras llega, y —esto es lo importante— saca **primero** las citas, porque el
//!    prompt las pide primero. Asi la capa de arriba puede verificar antes de ensenar una
//!    sola palabra: si las citas no valen, el usuario no llega a ver una respuesta
//!    inventada y luego retirada.

use serde::Serialize;
use serde_json::Value;

use crate::error::{AppError, AppResult};

/// Una cita tal y como la devuelve el modelo, sin verificar todavia.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RawCitation {
    /// Numero de fragmento, tal y como se le presento en el prompt. Empieza en 1.
    pub fragment: usize,
    /// Trozo literal del fragmento que respalda la afirmacion. Es lo que hace la cita
    /// comprobable: un numero solo dice a donde apunta, no que diga lo que se afirma.
    pub quote: String,
}

/// La respuesta del modelo ya parseada, antes de verificar las citas.
#[derive(Debug, Clone)]
pub struct StructuredAnswer {
    pub citations: Vec<RawCitation>,
    /// El modelo declara que no encuentra experiencia relevante. Es una senal util, pero
    /// **no** es la garantia de §6: un modelo puede decir que si y estarse inventando la
    /// experiencia. La garantia es la verificacion de las citas.
    pub answerable: bool,
    pub answer: String,
    pub key_points: Vec<String>,
    pub follow_ups: Vec<String>,
}

pub fn parse(raw: &str) -> AppResult<StructuredAnswer> {
    let object = extract_json_object(raw).ok_or_else(|| {
        AppError::Provider(format!(
            "el modelo no devolvio JSON. Empezaba asi: {}",
            raw.chars().take(120).collect::<String>()
        ))
    })?;

    let value: Value = serde_json::from_str(object)
        .map_err(|err| AppError::Provider(format!("el JSON del modelo no es valido: {err}")))?;

    let answer = field(&value, &["answer", "suggestedResponse", "suggested_response"])
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .ok_or_else(|| AppError::Provider("el modelo no devolvio ninguna respuesta".into()))?
        .to_owned();

    Ok(StructuredAnswer {
        citations: citations_from(field(&value, &["citations", "sources", "fuentes"])),
        // Ausente cuenta como `true`: el veredicto de §6 no depende de este campo, y un
        // modelo que se olvida de ponerlo no debe bloquear una respuesta bien citada.
        answerable: field(&value, &["answerable", "hasEvidence", "has_evidence"])
            .and_then(Value::as_bool)
            .unwrap_or(true),
        answer,
        key_points: string_list(field(&value, &["keyPoints", "key_points", "keypoints"])),
        follow_ups: string_list(field(&value, &["followUps", "follow_ups", "followups"])),
    })
}

/// Busca un campo por varios nombres posibles. Los modelos alternan entre `camelCase` y
/// `snake_case` con total indiferencia por lo que diga el prompt.
fn field<'a>(value: &'a Value, names: &[&str]) -> Option<&'a Value> {
    names.iter().find_map(|name| value.get(*name))
}

/// Acepta una lista de cadenas, una cadena suelta (se convierte en lista de uno) o nada.
fn string_list(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| match item {
                Value::String(text) => Some(text.trim().to_owned()),
                // Algun modelo devuelve `[{"point": "..."}]`. Se rescata el texto.
                Value::Object(_) => item
                    .as_object()
                    .and_then(|map| map.values().find_map(Value::as_str))
                    .map(|text| text.trim().to_owned()),
                _ => None,
            })
            .filter(|text| !text.is_empty())
            .collect(),
        Some(Value::String(text)) if !text.trim().is_empty() => vec![text.trim().to_owned()],
        _ => Vec::new(),
    }
}

/// Lee las citas admitiendo las tres formas que se ven en la practica:
/// `[{"fragment":1,"quote":"..."}]`, `[{"id":1,"text":"..."}]` y `[1, 2]`.
///
/// La ultima —un numero suelto, sin cita literal— se conserva a proposito en vez de
/// descartarla aqui: asi la verificacion la rechaza con un motivo concreto que se puede
/// ensenar, en vez de desaparecer sin dejar rastro.
fn citations_from(value: Option<&Value>) -> Vec<RawCitation> {
    let Some(Value::Array(items)) = value else {
        return Vec::new();
    };

    items
        .iter()
        .filter_map(|item| match item {
            Value::Number(number) => Some(RawCitation {
                fragment: usize::try_from(number.as_u64()?).ok()?,
                quote: String::new(),
            }),
            Value::Object(_) => {
                let fragment = field(item, &["fragment", "id", "index", "fragmentId"])?;
                let fragment = match fragment {
                    Value::Number(number) => usize::try_from(number.as_u64()?).ok()?,
                    // "[2]" o "2" en vez de 2.
                    Value::String(text) => text.trim().trim_matches(['[', ']']).parse().ok()?,
                    _ => return None,
                };
                Some(RawCitation {
                    fragment,
                    quote: field(item, &["quote", "text", "excerpt", "cita"])
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .trim()
                        .to_owned(),
                })
            }
            _ => None,
        })
        .collect()
}

/// Recorta el primer objeto JSON completo que haya en el texto, ignorando vallas de
/// codigo y cualquier prosa alrededor. Cuenta llaves respetando las cadenas, porque una
/// llave dentro de una cita del CV no abre ningun objeto.
fn extract_json_object(raw: &str) -> Option<&str> {
    let bytes = raw.as_bytes();
    let start = raw.find('{')?;

    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for index in start..bytes.len() {
        let byte = bytes[index];

        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }

        match byte {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&raw[start..=index]);
                }
            }
            _ => {}
        }
    }

    None
}

// ---------------------------------------------------------------------------
// Extractor incremental
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanEvent {
    /// Las citas ya estan completas. Llegan antes que la respuesta porque el prompt las
    /// pide primero, y eso es lo que permite verificar antes de ensenar nada.
    Citations(Vec<RawCitation>),
    /// El modelo ya ha dicho si encuentra experiencia relevante. Tambien llega antes que
    /// el texto: junto con las citas completa el veredicto de §6 sin haber ensenado nada.
    Answerable(bool),
    /// Un trozo mas del campo `answer`, ya sin comillas ni escapes.
    AnswerDelta(String),
    /// El campo `answer` se cerro.
    AnswerEnd,
}

/// Va sacando campos de un objeto JSON que todavia se esta escribiendo.
#[derive(Debug, Default)]
pub struct StreamScanner {
    buffer: String,
    citations_done: bool,
    answerable_done: bool,
    /// Indice en `buffer` del primer caracter del contenido de `answer`, una vez
    /// localizada la comilla que lo abre.
    answer_cursor: Option<usize>,
    answer_done: bool,
}

impl StreamScanner {
    /// Anade texto recien llegado y devuelve lo que ya se puede afirmar.
    pub fn push(&mut self, text: &str) -> Vec<ScanEvent> {
        self.buffer.push_str(text);
        let mut events = Vec::new();

        if !self.citations_done {
            if let Some(span) = top_level_value(&self.buffer, &["citations", "sources"]) {
                if let Some(array) = balanced_span(&self.buffer, span, b'[', b']') {
                    let parsed = serde_json::from_str::<Value>(array).ok();
                    self.citations_done = true;
                    events.push(ScanEvent::Citations(citations_from(parsed.as_ref())));
                }
            }
        }

        if !self.answerable_done {
            if let Some(span) = top_level_value(&self.buffer, &["answerable", "hasEvidence"]) {
                // `true` y `false` son literales cortos: en cuanto el buffer tiene uno
                // completo, la decision ya no puede cambiar.
                let rest = &self.buffer[span..];
                if rest.starts_with("true") {
                    self.answerable_done = true;
                    events.push(ScanEvent::Answerable(true));
                } else if rest.starts_with("false") {
                    self.answerable_done = true;
                    events.push(ScanEvent::Answerable(false));
                }
            }
        }

        if self.answer_done {
            return events;
        }

        if self.answer_cursor.is_none() {
            if let Some(span) = top_level_value(&self.buffer, &["answer", "suggestedResponse"]) {
                // El valor tiene que empezar por comilla; si aun no ha llegado, se espera.
                if self.buffer.as_bytes().get(span) == Some(&b'"') {
                    self.answer_cursor = Some(span + 1);
                }
            }
        }

        if let Some(cursor) = self.answer_cursor {
            let (decoded, consumed, closed) = decode_json_string(&self.buffer[cursor..]);
            if !decoded.is_empty() {
                events.push(ScanEvent::AnswerDelta(decoded));
            }
            self.answer_cursor = Some(cursor + consumed);
            if closed {
                self.answer_done = true;
                events.push(ScanEvent::AnswerEnd);
            }
        }

        events
    }
}

/// Localiza el valor de una clave de primer nivel y devuelve el indice donde empieza.
///
/// Recorre respetando cadenas y profundidad en vez de buscar la subcadena `"answer":`
/// directamente: si no, una cita del CV que contenga esas letras se llevaria por delante
/// el campo de verdad.
fn top_level_value(buffer: &str, names: &[&str]) -> Option<usize> {
    let bytes = buffer.as_bytes();
    let start = buffer.find('{')?;

    let mut depth = 0usize;
    let mut index = start;

    while index < bytes.len() {
        match bytes[index] {
            b'{' | b'[' => {
                depth += 1;
                index += 1;
            }
            b'}' | b']' => {
                depth = depth.saturating_sub(1);
                index += 1;
            }
            b'"' => {
                let (text, after) = read_json_string(buffer, index)?;
                // Solo las claves del objeto raiz interesan. Dentro de un objeto anidado,
                // profundidad 2 o mas, cualquier coincidencia es de otro campo.
                if depth == 1 && names.contains(&text.as_str()) {
                    let mut cursor = after;
                    while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
                        cursor += 1;
                    }
                    if bytes.get(cursor) == Some(&b':') {
                        cursor += 1;
                        while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
                            cursor += 1;
                        }
                        return (cursor < bytes.len()).then_some(cursor);
                    }
                }
                index = after;
            }
            _ => index += 1,
        }
    }

    None
}

/// Lee una cadena JSON completa a partir de la comilla de apertura. Devuelve el contenido
/// en crudo y el indice siguiente a la comilla de cierre, o `None` si aun no ha llegado.
fn read_json_string(buffer: &str, open_quote: usize) -> Option<(String, usize)> {
    let bytes = buffer.as_bytes();
    let mut index = open_quote + 1;
    let mut escaped = false;

    while index < bytes.len() {
        let byte = bytes[index];
        if escaped {
            escaped = false;
        } else if byte == b'\\' {
            escaped = true;
        } else if byte == b'"' {
            return Some((buffer[open_quote + 1..index].to_owned(), index + 1));
        }
        index += 1;
    }

    None
}

/// Devuelve el trozo `[...]` o `{...}` completo que empieza en `start`, si ya cerro.
fn balanced_span(buffer: &str, start: usize, open: u8, close: u8) -> Option<&str> {
    let bytes = buffer.as_bytes();
    if bytes.get(start) != Some(&open) {
        return None;
    }

    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for index in start..bytes.len() {
        let byte = bytes[index];
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        if byte == b'"' {
            in_string = true;
        } else if byte == open {
            depth += 1;
        } else if byte == close {
            depth -= 1;
            if depth == 0 {
                return Some(&buffer[start..=index]);
            }
        }
    }

    None
}

/// Decodifica lo que se pueda de una cadena JSON parcial.
///
/// Devuelve el texto ya sin escapes, cuantos bytes de la entrada se han consumido (para
/// no volver a emitirlos) y si se encontro la comilla de cierre. Se para en seco ante un
/// escape incompleto —`\` al final del trozo, o `\u00` a medias— porque el resto llega en
/// el siguiente evento del stream.
fn decode_json_string(input: &str) -> (String, usize, bool) {
    let bytes = input.as_bytes();
    let mut out = String::new();
    let mut index = 0usize;

    while index < bytes.len() {
        match bytes[index] {
            b'"' => return (out, index + 1, true),
            b'\\' => {
                let Some(escape) = bytes.get(index + 1) else {
                    // El escape se parte entre dos trozos del stream: se espera.
                    break;
                };
                match escape {
                    b'n' => out.push('\n'),
                    b't' => out.push('\t'),
                    b'r' => out.push('\r'),
                    b'b' => out.push('\u{8}'),
                    b'f' => out.push('\u{c}'),
                    b'u' => {
                        let Some(hex) = input.get(index + 2..index + 6) else {
                            break;
                        };
                        match u32::from_str_radix(hex, 16).ok().and_then(char::from_u32) {
                            Some(character) => out.push(character),
                            // Un sustituto suelto de UTF-16. Perder un caracter raro es
                            // mejor que abortar la respuesta entera.
                            None => out.push('\u{fffd}'),
                        }
                        index += 6;
                        continue;
                    }
                    // `\"`, `\\`, `\/` y cualquier otro: el propio caracter.
                    other => out.push(char::from(*other)),
                }
                index += 2;
            }
            _ => {
                // Avanzar por caracteres y no por bytes: un acento ocupa dos bytes y
                // partirlo produciria texto roto en la UI.
                let rest = &input[index..];
                let Some(character) = rest.chars().next() else {
                    break;
                };
                // Un caracter multibyte incompleto al final del trozo se deja para la
                // proxima vuelta. `from_utf8_lossy` ya lo habria sustituido antes de
                // llegar aqui, asi que esto solo protege de un corte limpio.
                out.push(character);
                index += character.len_utf8();
            }
        }
    }

    (out, index, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMPLETA: &str = r#"{
        "citations": [{"fragment": 2, "quote": "lidere la migracion"}],
        "answerable": true,
        "answer": "Lidere la migracion de un monolito a microservicios.",
        "keyPoints": ["Contexto", "Accion", "Resultado"],
        "followUps": ["Que aprendiste?"]
    }"#;

    #[test]
    fn parsea_la_respuesta_completa() {
        let parsed = parse(COMPLETA).expect("parsear");
        assert_eq!(parsed.citations.len(), 1);
        assert_eq!(parsed.citations[0].fragment, 2);
        assert_eq!(parsed.key_points.len(), 3);
        assert_eq!(parsed.follow_ups.len(), 1);
        assert!(parsed.answerable);
    }

    /// Los modelos meten el JSON dentro de una valla de codigo con una frase delante mas
    /// veces de las que lo devuelven limpio.
    #[test]
    fn aguanta_vallas_de_codigo_y_prosa_alrededor() {
        let sucio = format!("Claro, aqui tienes:\n```json\n{COMPLETA}\n```\nEspero que sirva.");
        let parsed = parse(&sucio).expect("parsear");
        assert_eq!(parsed.citations[0].fragment, 2);
    }

    /// Una llave dentro del texto de la respuesta no abre ningun objeto. Sin respetar las
    /// cadenas, el recorte se cierra donde no debe y el JSON sale invalido.
    #[test]
    fn una_llave_dentro_de_una_cadena_no_confunde_al_recorte() {
        let raw = r#"{"answer": "Use un mapa {clave: valor} en Rust", "citations": []}"#;
        let parsed = parse(raw).expect("parsear");
        assert!(parsed.answer.contains("{clave: valor}"));
    }

    #[test]
    fn acepta_snake_case_y_camel_case() {
        let raw = r#"{"answer":"x","key_points":["a"],"follow_ups":["b"],"citations":[]}"#;
        let parsed = parse(raw).expect("parsear");
        assert_eq!(parsed.key_points, vec!["a"]);
        assert_eq!(parsed.follow_ups, vec!["b"]);
    }

    #[test]
    fn una_cadena_donde_se_pidio_una_lista_se_convierte_en_lista_de_uno() {
        let raw = r#"{"answer":"x","keyPoints":"un solo punto","citations":[]}"#;
        assert_eq!(parse(raw).expect("parsear").key_points, vec!["un solo punto"]);
    }

    /// Un numero suelto es una cita sin nada que comprobar. Se conserva para que la
    /// verificacion la rechace con un motivo, en vez de desaparecer.
    #[test]
    fn una_cita_sin_texto_literal_se_conserva_vacia() {
        let raw = r#"{"answer":"x","citations":[3]}"#;
        let parsed = parse(raw).expect("parsear");
        assert_eq!(parsed.citations, vec![RawCitation { fragment: 3, quote: String::new() }]);
    }

    #[test]
    fn acepta_los_nombres_alternativos_de_las_citas() {
        let raw = r#"{"answer":"x","citations":[{"id":"[4]","text":"algo literal"}]}"#;
        let parsed = parse(raw).expect("parsear");
        assert_eq!(parsed.citations[0].fragment, 4);
        assert_eq!(parsed.citations[0].quote, "algo literal");
    }

    #[test]
    fn sin_respuesta_no_hay_nada_que_ensenar() {
        assert!(parse(r#"{"citations":[]}"#).is_err());
        assert!(parse(r#"{"answer":"   ","citations":[]}"#).is_err());
        assert!(parse("lo siento, no puedo ayudar con eso").is_err());
    }

    #[test]
    fn el_campo_answerable_ausente_no_bloquea_la_respuesta() {
        assert!(parse(r#"{"answer":"x","citations":[]}"#)
            .expect("parsear")
            .answerable);
    }

    // --- extractor incremental ---

    /// Trocea el texto en pedazos de `size` bytes respetando los limites de caracter,
    /// que es como llega de verdad desde el stream.
    fn en_trozos(text: &str, size: usize) -> Vec<String> {
        let chars: Vec<char> = text.chars().collect();
        chars
            .chunks(size)
            .map(|chunk| chunk.iter().collect())
            .collect()
    }

    fn recoger(events: &[ScanEvent]) -> String {
        events
            .iter()
            .filter_map(|event| match event {
                ScanEvent::AnswerDelta(text) => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn el_extractor_reconstruye_la_respuesta_trozo_a_trozo() {
        let mut scanner = StreamScanner::default();
        let mut todo = Vec::new();
        for chunk in en_trozos(COMPLETA, 7) {
            todo.extend(scanner.push(&chunk));
        }

        assert_eq!(
            recoger(&todo),
            "Lidere la migracion de un monolito a microservicios."
        );
        assert!(todo.contains(&ScanEvent::AnswerEnd));
    }

    /// La razon de ser del orden de los campos: si las citas no llegaran antes que la
    /// respuesta, habria que ensenar texto sin verificar y retirarlo despues.
    #[test]
    fn las_citas_llegan_antes_que_el_primer_trozo_de_respuesta() {
        let mut scanner = StreamScanner::default();
        let mut todo = Vec::new();
        for chunk in en_trozos(COMPLETA, 5) {
            todo.extend(scanner.push(&chunk));
        }

        let citas = todo
            .iter()
            .position(|event| matches!(event, ScanEvent::Citations(_)))
            .expect("deberia haber citas");
        let primer_texto = todo
            .iter()
            .position(|event| matches!(event, ScanEvent::AnswerDelta(_)))
            .expect("deberia haber respuesta");

        assert!(
            citas < primer_texto,
            "las citas tienen que poder verificarse antes de ensenar nada"
        );
    }

    #[test]
    fn el_extractor_deshace_los_escapes() {
        let mut scanner = StreamScanner::default();
        let raw = r#"{"citations":[],"answer":"Dijo \"hola\"\ny se fue. 100%"}"#;
        let mut todo = Vec::new();
        for chunk in en_trozos(raw, 3) {
            todo.extend(scanner.push(&chunk));
        }
        assert_eq!(recoger(&todo), "Dijo \"hola\"\ny se fue. 100%");
    }

    /// Los acentos ocupan dos bytes. Partirlos daria texto roto en pantalla, que es
    /// justamente el defecto que ya aparecio en la extraccion de PDF.
    #[test]
    fn los_acentos_no_se_parten_por_la_mitad() {
        let mut scanner = StreamScanner::default();
        let raw = r#"{"citations":[],"answer":"Diseñé la migración día a día"}"#;
        let mut todo = Vec::new();
        for chunk in en_trozos(raw, 1) {
            todo.extend(scanner.push(&chunk));
        }
        assert_eq!(recoger(&todo), "Diseñé la migración día a día");
    }

    /// Si el modelo se salta el orden pedido, el extractor sigue funcionando: la capa de
    /// arriba es la que retiene el texto hasta tener citas verificadas.
    #[test]
    fn tambien_funciona_si_el_modelo_invierte_el_orden() {
        let mut scanner = StreamScanner::default();
        let raw = r#"{"answer":"respuesta primero","citations":[{"fragment":1,"quote":"q"}]}"#;
        let mut todo = Vec::new();
        for chunk in en_trozos(raw, 9) {
            todo.extend(scanner.push(&chunk));
        }

        assert_eq!(recoger(&todo), "respuesta primero");
        assert!(todo
            .iter()
            .any(|event| matches!(event, ScanEvent::Citations(citations) if citations.len() == 1)));
    }

    /// Una cita del CV que contenga la palabra `answer` entre comillas no debe hacerse
    /// pasar por el campo `answer`.
    #[test]
    fn una_clave_falsa_dentro_de_una_cadena_no_engana_al_extractor() {
        let mut scanner = StreamScanner::default();
        let raw = r#"{"citations":[{"fragment":1,"quote":"the \"answer\": 42"}],"answer":"real"}"#;
        let mut todo = Vec::new();
        for chunk in en_trozos(raw, 11) {
            todo.extend(scanner.push(&chunk));
        }
        assert_eq!(recoger(&todo), "real");
    }

    #[test]
    fn un_json_a_medias_no_emite_nada_todavia() {
        let mut scanner = StreamScanner::default();
        let events = scanner.push(r#"{"citations":[{"fragment":1,"#);
        assert!(events.is_empty());
    }
}
