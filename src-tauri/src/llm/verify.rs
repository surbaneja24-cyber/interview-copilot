//! Verificacion de citas: la implementacion de §6.
//!
//! El requisito es que la IA no invente experiencia del candidato. El intento anterior
//! —un umbral sobre la similitud del retriever— se midio y **no funciona**: las nubes de
//! preguntas con respuesta y sin respuesta se solapan siempre, porque la similitud mide
//! de que habla un texto, no si responde a otro (`docs/ARCHITECTURE.md` §5). No se
//! reabre sin volver a medir.
//!
//! Lo que si es comprobable por una maquina es esto:
//!
//! 1. El fragmento citado tiene que ser uno de los que se enviaron.
//! 2. El texto citado tiene que estar **literalmente** dentro de ese fragmento.
//!
//! La segunda condicion es la que aguanta el peso. Comprobar solo la primera seria casi
//! decorativo: con cinco fragmentos numerados, un modelo que se invente una experiencia
//! escribe igualmente `"fragment": 1` y la cita "existe". Exigir una copia literal es
//! otra cosa: para pasar el filtro, el modelo tiene que haber copiado palabras que de
//! verdad estan en los documentos del candidato.
//!
//! **Los dos fallos no se tratan igual, y la asimetria es deliberada:**
//!
//! - Un fragmento que nunca se envio es una referencia inventada. No es un desliz de
//!   redaccion: es el modelo fabricando respaldo. Tumba la respuesta entera.
//! - Una cita literal que no aparece suele ser una parafrasis, que es un defecto mucho
//!   mas benigno. Se cae esa cita sola. Si no sobrevive ninguna, no hay respuesta.
//!
//! **Lo que esto NO garantiza,** y conviene tenerlo escrito para no venderlo de mas: que
//! el fragmento citado respalde *lo que la respuesta afirma*. Un modelo podria copiar una
//! frase real del CV y colgarle al lado una afirmacion inventada. Contra eso no hay
//! comprobacion mecanica; lo que hay es que la UI ensena la cita al lado de la respuesta
//! para que el candidato lo vea de un vistazo (§6 del spec).

use serde::Serialize;

use crate::llm::answer::{RawCitation, StructuredAnswer};
use crate::llm::prompt::FragmentSet;

/// Una cita que paso las dos comprobaciones.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifiedCitation {
    pub fragment: usize,
    pub chunk_id: i64,
    pub document_title: String,
    /// El texto tal y como lo escribio el modelo. Se ensena en la UI.
    pub quote: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DropReason {
    /// El modelo dio el numero de fragmento pero no copio nada de el.
    EmptyQuote,
    /// Lo que cito no esta en el fragmento. Casi siempre, una parafrasis.
    QuoteNotFound,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DroppedCitation {
    pub fragment: usize,
    pub quote: String,
    pub reason: DropReason,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "reason", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Unsupported {
    /// No habia nada indexado que enviar. No es que el modelo fallara: es que no se le
    /// dio con que responder.
    NoContext,
    /// El modelo declaro que no encuentra experiencia relevante.
    ModelFoundNothing,
    /// No cito nada.
    NoCitations,
    /// Cito un fragmento que nunca se le envio: referencia inventada.
    InventedFragment { fragment: usize },
    /// Cito, pero ninguna cita esta literalmente en los documentos.
    NoLiteralSupport { dropped: Vec<DroppedCitation> },
}

impl Unsupported {
    /// Explicacion para la UI. Va en el mismo idioma que el resto de la aplicacion.
    pub fn explain(&self) -> String {
        match self {
            Self::NoContext => {
                "No hay ningun documento indexado en este proyecto, asi que no hay nada en \
                 lo que basar una respuesta. Carga tu CV en Preparacion."
                    .into()
            }
            Self::ModelFoundNothing => {
                "El modelo no ha encontrado en tus documentos ninguna experiencia que \
                 responda a esta pregunta."
                    .into()
            }
            Self::NoCitations => {
                "La respuesta no venia respaldada por ningun fragmento de tus documentos, \
                 asi que se ha descartado."
                    .into()
            }
            Self::InventedFragment { fragment } => format!(
                "La respuesta citaba un fragmento ({fragment}) que no existe entre los que \
                 se le enviaron. Se ha descartado por completo: una referencia inventada \
                 invalida el resto."
            ),
            Self::NoLiteralSupport { dropped } => format!(
                "Ninguna de las {} citas aparece literalmente en tus documentos, asi que la \
                 respuesta se ha descartado.",
                dropped.len()
            ),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Verdict {
    Supported {
        citations: Vec<VerifiedCitation>,
        /// Citas caidas por no ser literales. Se conservan para poder ver, con un modelo
        /// real, cuanto parafrasea antes de decidir si el filtro es demasiado estricto.
        dropped: Vec<DroppedCitation>,
    },
    Unsupported(Unsupported),
}

pub fn verify(answer: &StructuredAnswer, fragments: &FragmentSet) -> Verdict {
    if !answer.answerable {
        return Verdict::Unsupported(Unsupported::ModelFoundNothing);
    }

    check_citations(&answer.citations, fragments)
}

/// Comprueba solo las citas, sin mirar el resto de la respuesta.
///
/// Esta separado de `verify` porque durante el streaming las citas llegan antes que el
/// texto —el prompt las pide primero— y esto es lo que permite decidir si se puede
/// empezar a ensenar la respuesta cuando el modelo todavia la esta escribiendo.
pub fn check_citations(citations: &[RawCitation], fragments: &FragmentSet) -> Verdict {
    if citations.is_empty() {
        return Verdict::Unsupported(Unsupported::NoCitations);
    }

    // Primero el fallo grave: una sola referencia inventada tumba la respuesta entera,
    // aunque las demas citas fueran perfectas.
    for citation in citations {
        if fragments.get(citation.fragment).is_none() {
            return Verdict::Unsupported(Unsupported::InventedFragment {
                fragment: citation.fragment,
            });
        }
    }

    let mut verified = Vec::new();
    let mut dropped = Vec::new();

    for citation in citations {
        let Some(fragment) = fragments.get(citation.fragment) else {
            continue; // Imposible: el bucle de arriba ya habria salido.
        };

        match check_quote(citation, &fragment.text) {
            Ok(()) => verified.push(VerifiedCitation {
                fragment: citation.fragment,
                chunk_id: fragment.chunk_id,
                document_title: fragment.document_title.clone(),
                quote: citation.quote.clone(),
            }),
            Err(reason) => dropped.push(DroppedCitation {
                fragment: citation.fragment,
                quote: citation.quote.clone(),
                reason,
            }),
        }
    }

    if verified.is_empty() {
        return Verdict::Unsupported(Unsupported::NoLiteralSupport { dropped });
    }

    Verdict::Supported {
        citations: verified,
        dropped,
    }
}

fn check_quote(citation: &RawCitation, fragment_text: &str) -> Result<(), DropReason> {
    if citation.quote.trim().is_empty() {
        return Err(DropReason::EmptyQuote);
    }

    if contains_quote(fragment_text, &citation.quote) {
        Ok(())
    } else {
        Err(DropReason::QuoteNotFound)
    }
}

/// Comprueba que la cita este en el fragmento.
///
/// "Literal" se entiende con dos indulgencias, y solo dos:
///
/// - **Mayusculas y espacios no cuentan.** Un salto de linea del PDF convertido en espacio
///   no es una invencion. Los acentos y la puntuacion si cuentan: en cuanto se empiezan a
///   ignorar, "literal" deja de significar nada.
/// - **Los puntos suspensivos parten la cita.** Los modelos recortan por el medio con
///   `...`; cada trozo tiene que aparecer, y en orden.
fn contains_quote(fragment_text: &str, quote: &str) -> bool {
    let haystack = normalize(fragment_text);

    let mut cursor = 0usize;
    let mut checked_any = false;

    for piece in split_on_ellipsis(quote) {
        let needle = normalize(piece);
        if needle.is_empty() {
            continue;
        }
        checked_any = true;

        let Some(found) = haystack[cursor..].find(&needle) else {
            return false;
        };
        cursor += found + needle.len();
    }

    checked_any
}

fn split_on_ellipsis(quote: &str) -> Vec<&str> {
    quote
        .split('\u{2026}')
        .flat_map(|part| part.split("..."))
        .collect()
}

/// Minusculas y espacios colapsados. Nada mas.
fn normalize(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rag::retriever::RetrievedChunk;
    use crate::storage::{DocumentKind, StoredChunk};

    const TEXTO: &str = "Lideré la migración de un monolito a microservicios en Acme,\n\
                         reduciendo el tiempo de despliegue de 40 a 6 minutos.";

    fn fragmentos() -> FragmentSet {
        FragmentSet::from_retrieval(&[RetrievedChunk {
            chunk: StoredChunk {
                id: 412,
                document_id: 1,
                document_title: "CV.docx".into(),
                kind: DocumentKind::Cv,
                ordinal: 0,
                text: TEXTO.into(),
            },
            similarity: 0.85,
        }])
    }

    fn respuesta(citations: Vec<RawCitation>, answerable: bool) -> StructuredAnswer {
        StructuredAnswer {
            citations,
            answerable,
            answer: "Lideré una migración a microservicios.".into(),
            key_points: vec!["Contexto".into()],
            follow_ups: vec!["¿Qué aprendiste?".into()],
        }
    }

    fn cita(fragment: usize, quote: &str) -> RawCitation {
        RawCitation {
            fragment,
            quote: quote.into(),
        }
    }

    #[test]
    fn una_cita_literal_respalda_la_respuesta() {
        let answer = respuesta(vec![cita(1, "reduciendo el tiempo de despliegue")], true);
        match verify(&answer, &fragmentos()) {
            Verdict::Supported { citations, dropped } => {
                assert_eq!(citations.len(), 1);
                assert_eq!(citations[0].chunk_id, 412);
                assert!(dropped.is_empty());
            }
            Verdict::Unsupported(reason) => panic!("deberia valer: {reason:?}"),
        }
    }

    /// El salto de linea del documento original no puede invalidar una cita correcta.
    #[test]
    fn los_saltos_de_linea_y_las_mayusculas_no_invalidan_una_cita() {
        let answer = respuesta(
            vec![cita(1, "EN ACME,   reduciendo el tiempo")],
            true,
        );
        assert!(matches!(
            verify(&answer, &fragmentos()),
            Verdict::Supported { .. }
        ));
    }

    /// Los acentos si cuentan. Si se ignoran, "literal" empieza a significar
    /// "aproximadamente", y ese es el camino que ya se demostro que no lleva a ningun
    /// sitio con los umbrales de similitud.
    #[test]
    fn quitar_los_acentos_no_cuela() {
        let answer = respuesta(vec![cita(1, "Lidere la migracion")], true);
        assert!(matches!(
            verify(&answer, &fragmentos()),
            Verdict::Unsupported(Unsupported::NoLiteralSupport { .. })
        ));
    }

    #[test]
    fn los_puntos_suspensivos_parten_la_cita_en_trozos() {
        let answer = respuesta(vec![cita(1, "Lideré la migración ... 40 a 6 minutos")], true);
        assert!(matches!(
            verify(&answer, &fragmentos()),
            Verdict::Supported { .. }
        ));
    }

    /// El orden importa: los trozos tienen que aparecer como aparecen en el documento.
    #[test]
    fn los_trozos_desordenados_no_valen() {
        let answer = respuesta(vec![cita(1, "40 a 6 minutos ... Lideré la migración")], true);
        assert!(matches!(
            verify(&answer, &fragmentos()),
            Verdict::Unsupported(_)
        ));
    }

    /// El caso que da sentido a todo el modulo: el modelo se inventa una experiencia y la
    /// respalda con una frase que no esta en ninguna parte.
    #[test]
    fn una_experiencia_inventada_no_pasa_el_filtro() {
        let answer = respuesta(
            vec![cita(1, "dirigí un equipo de ventas de 20 personas")],
            true,
        );
        match verify(&answer, &fragmentos()) {
            Verdict::Unsupported(Unsupported::NoLiteralSupport { dropped }) => {
                assert_eq!(dropped[0].reason, DropReason::QuoteNotFound);
            }
            other => panic!("deberia rechazarse: {other:?}"),
        }
    }

    /// Una referencia a un fragmento que nunca se envio tumba la respuesta entera, aunque
    /// venga acompanada de una cita impecable.
    #[test]
    fn un_fragmento_inventado_tumba_toda_la_respuesta() {
        let answer = respuesta(
            vec![
                cita(1, "reduciendo el tiempo de despliegue"),
                cita(7, "algo que dice el fragmento siete"),
            ],
            true,
        );
        assert!(matches!(
            verify(&answer, &fragmentos()),
            Verdict::Unsupported(Unsupported::InventedFragment { fragment: 7 })
        ));
    }

    /// Una cita parafraseada entre varias correctas solo se cae ella. Parafrasear es un
    /// defecto de redaccion, no una invencion.
    #[test]
    fn una_parafrasis_entre_citas_buenas_solo_se_cae_ella() {
        let answer = respuesta(
            vec![
                cita(1, "reduciendo el tiempo de despliegue"),
                cita(1, "mejoré mucho los despliegues"),
            ],
            true,
        );
        match verify(&answer, &fragmentos()) {
            Verdict::Supported { citations, dropped } => {
                assert_eq!(citations.len(), 1);
                assert_eq!(dropped.len(), 1);
            }
            other => panic!("deberia valer con una sola cita: {other:?}"),
        }
    }

    #[test]
    fn un_numero_de_fragmento_sin_texto_citado_no_respalda_nada() {
        let answer = respuesta(vec![cita(1, "")], true);
        match verify(&answer, &fragmentos()) {
            Verdict::Unsupported(Unsupported::NoLiteralSupport { dropped }) => {
                assert_eq!(dropped[0].reason, DropReason::EmptyQuote);
            }
            other => panic!("deberia rechazarse: {other:?}"),
        }
    }

    #[test]
    fn sin_citas_no_hay_respuesta() {
        assert!(matches!(
            verify(&respuesta(Vec::new(), true), &fragmentos()),
            Verdict::Unsupported(Unsupported::NoCitations)
        ));
    }

    /// Si el modelo declara que no hay experiencia, se le cree aunque haya adjuntado
    /// citas: la contradiccion se resuelve siempre del lado de no afirmar nada.
    #[test]
    fn si_el_modelo_dice_que_no_hay_experiencia_se_le_cree() {
        let answer = respuesta(vec![cita(1, "reduciendo el tiempo de despliegue")], false);
        assert!(matches!(
            verify(&answer, &fragmentos()),
            Verdict::Unsupported(Unsupported::ModelFoundNothing)
        ));
    }

    /// Sin fragmentos recuperados no hay nada que citar, asi que cualquier cita es
    /// inventada por definicion.
    #[test]
    fn sin_fragmentos_ninguna_cita_es_valida() {
        let vacio = FragmentSet::default();
        let answer = respuesta(vec![cita(1, "lo que sea")], true);
        assert!(matches!(
            verify(&answer, &vacio),
            Verdict::Unsupported(Unsupported::InventedFragment { fragment: 1 })
        ));
    }

    /// Una cita de solo puntos suspensivos no comprueba nada, y sin esto pasaria el
    /// filtro por no tener ningun trozo que buscar.
    #[test]
    fn una_cita_de_solo_puntos_suspensivos_no_comprueba_nada() {
        let answer = respuesta(vec![cita(1, "...")], true);
        assert!(matches!(
            verify(&answer, &fragmentos()),
            Verdict::Unsupported(Unsupported::NoLiteralSupport { .. })
        ));
    }
}
