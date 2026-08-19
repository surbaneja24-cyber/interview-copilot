//! Proveedor simulado, solo en compilaciones de desarrollo.
//!
//! No usa ninguna IA: fabrica una respuesta con el formato correcto a partir de los
//! propios fragmentos recuperados. Sirve para recorrer la ruta completa sin instalar
//! nada, y para provocar a voluntad el caso de "no hay experiencia que citar" —basta con
//! empezar la pregunta por `!`.
//!
//! Va detras de `#[cfg(debug_assertions)]` porque un proveedor que devuelve texto
//! plausible sin haber consultado nada no puede acabar en manos de nadie en mitad de una
//! entrevista.
//!
//! No usa ninguna IA: fabrica una respuesta con el formato de §8 a partir de los propios
//! fragmentos que se le pasan. Sirve para dos cosas concretas:
//!
//! - Recorrer la ruta completa —recuperacion, streaming, compuerta, citas en pantalla—
//!   sin tener que instalar Ollama ni gastar una clave de API.
//! - Provocar a voluntad el caso de §6, que con un modelo de verdad es dificil de
//!   reproducir cuando toca: basta con empezar la pregunta por `!`.
//!
//! Va detras de `#[cfg(debug_assertions)]` a proposito. Un proveedor que devuelve texto
//! plausible sin haber consultado nada es exactamente lo que no puede acabar en manos de
//! alguien en mitad de una entrevista.

use tokio::sync::mpsc::UnboundedSender;

use crate::error::AppResult;
use crate::llm::settings::ProviderKind;
use crate::llm::{BoxFuture, ChatRequest, LlmProvider, ProviderDescription};

pub struct MockProvider;

impl LlmProvider for MockProvider {
    fn describe(&self) -> ProviderDescription {
        ProviderDescription {
            kind: ProviderKind::Mock,
            model: "simulador".into(),
            endpoint: "(ninguno)".into(),
            sends_data_outside: false,
        }
    }

    fn models(&self) -> BoxFuture<'_, AppResult<Vec<String>>> {
        Box::pin(async { Ok(vec!["simulador".to_owned()]) })
    }

    fn stream_chat(
        &self,
        request: ChatRequest,
        tokens: UnboundedSender<String>,
    ) -> BoxFuture<'_, AppResult<String>> {
        Box::pin(async move {
            let user = request
                .messages
                .last()
                .map(|message| message.content.as_str())
                .unwrap_or_default();

            let full = compose(user);

            // Trozos pequenos para que se vea que el texto llega escribiendose y no de
            // golpe, que es justo lo que hay que poder comprobar con los ojos.
            for chunk in full.as_bytes().chunks(24) {
                let piece = String::from_utf8_lossy(chunk).to_string();
                let _ = tokens.send(piece);
            }

            Ok(full)
        })
    }
}

/// Fabrica la respuesta. Si la pregunta empieza por `!`, simula un modelo que se inventa
/// la experiencia: cita un fragmento real pero con un texto que no esta en el.
fn compose(user_message: &str) -> String {
    let quote = first_words_of_fragment_one(user_message)
        .unwrap_or_else(|| "sin fragmentos".to_owned());
    let inventing = user_message
        .rsplit("PREGUNTA DEL ENTREVISTADOR")
        .next()
        .is_some_and(|question| question.trim_start().starts_with('!'));

    let (fragment, quote) = if inventing {
        (1, "dirigi un equipo de ventas de veinte personas".to_owned())
    } else {
        (1, quote)
    };

    let answer = if inventing {
        "Dirigi un equipo de ventas de veinte personas durante tres anos."
    } else {
        "Respuesta simulada: esto viene del proveedor de prueba, no de ninguna IA. \
         Se apoya en un trozo literal de tus documentos para que la verificacion de citas \
         se pueda comprobar de verdad."
    };

    serde_json::json!({
        "citations": [{ "fragment": fragment, "quote": quote }],
        "answerable": true,
        "answer": answer,
        "keyPoints": [
            "Este texto no lo ha escrito un modelo de lenguaje",
            "Sirve para comprobar el streaming y las citas",
            "Empieza la pregunta por ! para simular una experiencia inventada",
        ],
        "followUps": [
            "Que herramientas usaste?",
            "Que harias distinto?",
        ],
    })
    .to_string()
}

/// Saca unas cuantas palabras literales del fragmento [1] del prompt, para que la cita
/// que devuelve el simulador pase la verificacion de verdad y no por una excepcion.
fn first_words_of_fragment_one(user_message: &str) -> Option<String> {
    let start = user_message.find("[1] ")?;
    let rest = &user_message[start..];
    // La primera linea es el encabezado con el titulo del documento; el texto viene
    // debajo.
    let body = rest.split_once('\n')?.1;
    let body = body.split("\n\n").next()?;

    let words: Vec<&str> = body.split_whitespace().take(6).collect();
    (!words.is_empty()).then(|| words.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::prompt::{self, AnswerStyle, FragmentSet};
    use crate::llm::{answer, verify};
    use crate::rag::retriever::RetrievedChunk;
    use crate::storage::{DocumentKind, StoredChunk};

    fn fragmentos() -> FragmentSet {
        FragmentSet::from_retrieval(&[RetrievedChunk {
            chunk: StoredChunk {
                id: 412,
                document_id: 1,
                document_title: "CV.docx".into(),
                kind: DocumentKind::Cv,
                ordinal: 0,
                text: "Lideré la migración de un monolito a microservicios en Acme.".into(),
            },
            similarity: 0.85,
        }])
    }

    fn respuesta_para(question: &str) -> String {
        let messages = prompt::build(question, AnswerStyle::Behavioral, &fragmentos());
        compose(&messages[1].content)
    }

    /// El simulador solo vale si su respuesta pasa la misma verificacion que la de un
    /// modelo real. Si no, no esta ejercitando la ruta que dice ejercitar.
    #[test]
    fn la_respuesta_simulada_pasa_la_verificacion() {
        let parsed = answer::parse(&respuesta_para("Cuentame un proyecto")).expect("parsear");
        assert!(matches!(
            verify::verify(&parsed, &fragmentos()),
            verify::Verdict::Supported { .. }
        ));
    }

    #[test]
    fn con_admiracion_delante_simula_una_experiencia_inventada() {
        let parsed = answer::parse(&respuesta_para("!Cuentame de ventas")).expect("parsear");
        assert!(matches!(
            verify::verify(&parsed, &fragmentos()),
            verify::Verdict::Unsupported(_)
        ));
    }
}
