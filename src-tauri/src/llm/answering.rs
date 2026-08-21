//! Orquestacion: pregunta, contexto, generacion, verificacion, pantalla.
//!
//! El detalle que ordena todo lo demas es cuando se puede ensenar texto. La respuesta
//! llega token a token para que la latencia percibida sea baja, pero no puede mostrarse
//! nada que no este respaldado. Como el prompt exige las citas antes que el texto,
//! cuando empieza a llegar la respuesta ya se sabe si vale; hasta entonces `Gate` la
//! retiene. Nunca se ensena algo que luego haya que retirar.
//!
//! El detalle que ordena todo lo demas es **cuando se puede ensenar texto**. La respuesta
//! llega token a token para que la latencia percibida sea baja (§10), pero §6 prohibe
//! ensenar una experiencia que no este respaldada. Las dos cosas parecen incompatibles y
//! no lo son: el prompt obliga al modelo a escribir primero las citas y el veredicto de
//! `answerable`, asi que cuando empieza a llegar el texto de la respuesta ya se sabe si
//! vale. Hasta ese momento el texto se retiene; nunca se ensena algo que luego haya que
//! retirar.
//!
//! El veredicto que se emite al final no es el del streaming: se vuelve a parsear la
//! respuesta completa y se verifica otra vez. El del streaming decide si se puede ir
//! mostrando; el final es el que manda.

use std::time::Instant;

use serde::Serialize;
use tokio::sync::mpsc::unbounded_channel;

use crate::embedding::EmbeddingProvider;
use crate::error::{AppError, AppResult};
use crate::llm::answer::{self, ScanEvent, StreamScanner};
use crate::llm::prompt::{self, AnswerStyle, FragmentSet};
use crate::llm::verify::{self, DroppedCitation, Unsupported, Verdict, VerifiedCitation};
use crate::llm::{ChatRequest, LlmProvider, LlmSettings};
use crate::rag::retriever::{Material, Retriever, DEFAULT_TOP_K};
use crate::storage::Db;

/// Lo que la UI sabe de un fragmento enviado. Es deliberadamente escueto: §31 pide no
/// ensenar mas datos personales de los necesarios durante una entrevista, y el texto
/// completo del fragmento ya se puede consultar en Preparacion.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FragmentSummary {
    pub number: usize,
    pub document_title: String,
    pub preview: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum AnswerEvent {
    /// Que fragmentos se han recuperado y se van a enviar al modelo. Se emite antes de
    /// llamar a nadie: en modo nube, es lo que el usuario tiene derecho a saber que sale
    /// de su equipo (§15).
    Retrieved {
        fragments: Vec<FragmentSummary>,
        /// A donde se envia. Cadena vacia si no sale del equipo.
        sent_to: String,
    },
    /// Un trozo mas de respuesta, ya verificada.
    Delta { text: String },
    Completed {
        answer: String,
        key_points: Vec<String>,
        follow_ups: Vec<String>,
        citations: Vec<VerifiedCitation>,
        /// Citas que se cayeron por no ser literales. La UI las puede ensenar en pequeno;
        /// sirven para saber cuanto parafrasea el modelo que se este usando.
        dropped: Vec<DroppedCitation>,
        elapsed_ms: u64,
    },
    /// El aviso de §6. Nunca lleva respuesta, solo un esqueleto de como estructurarla.
    Unsupported {
        explanation: String,
        detail: Unsupported,
        structure: Vec<String>,
    },
    Failed { message: String },
}

/// Todo lo que hace falta para contestar, junto para no arrastrar ocho parametros por
/// cada paso.
pub struct Answering<'a> {
    pub db: &'a Db,
    pub embedder: &'a dyn EmbeddingProvider,
    pub provider: &'a dyn LlmProvider,
    pub settings: &'a LlmSettings,
}

impl Answering<'_> {
    /// Contesta una pregunta. `emit` recibe los eventos segun se producen: en la
    /// aplicacion los manda por un canal a la UI, y en los tests los guarda en un vector.
    pub async fn answer(
        &self,
        project_id: i64,
        question: &str,
        style: AnswerStyle,
        emit: &mut (dyn FnMut(AnswerEvent) + Send),
    ) -> AppResult<()> {
        let started = Instant::now();

        let fragments = self.retrieve(project_id, question, style)?;
        if fragments.is_empty() {
            emit(unsupported_event(Unsupported::NoContext, style));
            return Ok(());
        }

        emit(self.retrieved_event(&fragments));

        let raw = match self.generate(question, style, &fragments, emit).await {
            Ok(raw) => raw,
            Err(err) => return emit_failure(err, emit),
        };

        // Solo en desarrollo: la respuesta en crudo lleva dentro trozos del CV del
        // usuario y no tiene por que acabar en ningun log de una version distribuida.
        // Mientras se calibra el filtro de citas es lo unico que permite distinguir un
        // modelo que parafrasea de uno que se inventa el formato.
        #[cfg(debug_assertions)]
        log::info!("respuesta cruda del modelo: {raw}");

        // Veredicto definitivo sobre la respuesta completa. El del streaming solo servia
        // para decidir si se podia ir ensenando.
        let parsed = match answer::parse(&raw) {
            Ok(parsed) => parsed,
            Err(err) => return emit_failure(err, emit),
        };

        match verify::verify(&parsed, &fragments) {
            Verdict::Supported { citations, dropped } => emit(AnswerEvent::Completed {
                answer: parsed.answer,
                key_points: parsed.key_points,
                follow_ups: parsed.follow_ups,
                citations,
                dropped,
                elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            }),
            Verdict::Unsupported(reason) => {
                log::info!("respuesta descartada por falta de respaldo: {reason:?}");
                emit(unsupported_event(reason, style));
            }
        }

        Ok(())
    }

    fn retrieve(
        &self,
        project_id: i64,
        question: &str,
        style: AnswerStyle,
    ) -> AppResult<FragmentSet> {
        let retrieval = Retriever::new(self.db, self.embedder).search(
            project_id,
            question,
            DEFAULT_TOP_K,
            material_for(style),
        )?;

        Ok(FragmentSet::from_retrieval(&retrieval.chunks))
    }

    /// Se emite antes de llamar a nadie: en modo nube, es lo que el usuario tiene derecho
    /// a saber que sale de su equipo.
    fn retrieved_event(&self, fragments: &FragmentSet) -> AnswerEvent {
        let description = self.provider.describe();

        AnswerEvent::Retrieved {
            fragments: fragments
                .all()
                .iter()
                .map(|fragment| FragmentSummary {
                    number: fragment.number,
                    document_title: fragment.document_title.clone(),
                    preview: preview_of(&fragment.text),
                })
                .collect(),
            sent_to: if description.sends_data_outside {
                description.endpoint
            } else {
                String::new()
            },
        }
    }

    /// Genera y va soltando por `emit` el texto que la compuerta deja pasar. Devuelve la
    /// respuesta completa en crudo, que es sobre la que se dicta el veredicto final.
    async fn generate(
        &self,
        question: &str,
        style: AnswerStyle,
        fragments: &FragmentSet,
        emit: &mut (dyn FnMut(AnswerEvent) + Send),
    ) -> AppResult<String> {
        let request = ChatRequest {
            messages: prompt::build(question, style, fragments),
            temperature: self.settings.temperature,
            max_tokens: self.settings.max_tokens,
            json_mode: self.settings.json_mode,
        };

        let (tx, mut rx) = unbounded_channel();
        let mut gate = Gate::default();

        // Las dos mitades corren a la vez: una habla con el servidor y la otra va sacando
        // campos del JSON segun llegan. Sin esto no hay streaming, solo espera.
        let (generated, ()) =
            futures_util::future::join(self.provider.stream_chat(request, tx), async {
                while let Some(chunk) = rx.recv().await {
                    for event in gate.scanner.push(&chunk) {
                        gate.handle(event, fragments, emit);
                    }
                }
            })
            .await;

        generated
    }
}

/// Un fallo del proveedor o un JSON ilegible no son un error de la aplicacion: son algo
/// que contarle al usuario. Por eso se emiten y se devuelve `Ok`.
fn emit_failure(err: AppError, emit: &mut (dyn FnMut(AnswerEvent) + Send)) -> AppResult<()> {
    emit(AnswerEvent::Failed {
        message: err.to_string(),
    });
    Ok(())
}

/// Que material puede sostener una respuesta de este estilo.
///
/// Medido el 2026-08-20 (`ARCHITECTURE.md` §5.2): con una oferta indexada junto al CV, la
/// oferta entra en el top 5 de 19 de las 20 preguntas del banco y es la primera en 12. Ante
/// "cuentame un proyecto complicado", el modelo recibe como mejor prueba de la experiencia
/// del candidato un documento que dice lo que la empresa busca.
///
/// - `Behavioral` cuenta algo que paso. Si no salio del candidato, no paso.
/// - `Technical` es el caso mas peligroso de los tres, y por eso no se deja abierto: una
///   oferta enumera justo las herramientas que pide, asi que dejarla entrar es servirle al
///   modelo la lista de lo que le conviene decir que sabe.
/// - `General` se queda como estaba, admitiendo todo. Es el cajon de sastre de los tres:
///   ahi caen tanto "cuentame sobre ti" como "¿por que quieres trabajar aqui?", y esa
///   segunda **necesita** la oferta. Elegir un lado seria adivinar cual de las dos tenia el
///   usuario en la cabeza, y no hay nada medido que lo diga. Lo resuelve el clasificador de
///   §7, que si distingue por tipo de pregunta.
fn material_for(style: AnswerStyle) -> Material {
    match style {
        AnswerStyle::Behavioral | AnswerStyle::Technical => Material::CandidateOnly,
        AnswerStyle::General => Material::All,
    }
}

fn unsupported_event(reason: Unsupported, style: AnswerStyle) -> AnswerEvent {
    AnswerEvent::Unsupported {
        explanation: reason.explain(),
        detail: reason,
        structure: prompt::structure_hint(style),
    }
}

/// Primeras palabras de un fragmento, para que la UI pueda identificarlo sin volcar el
/// documento entero en pantalla (§31).
fn preview_of(text: &str) -> String {
    let trimmed = text.trim();
    let cut = trimmed
        .char_indices()
        .nth(120)
        .map_or(trimmed.len(), |(index, _)| index);

    if cut < trimmed.len() {
        format!("{}…", &trimmed[..cut])
    } else {
        trimmed.to_owned()
    }
}

/// La compuerta que retiene el texto hasta que hay con que respaldarlo.
#[derive(Default)]
struct Gate {
    scanner: StreamScanner,
    /// `None` mientras no han llegado las citas.
    citations_ok: Option<bool>,
    /// El modelo dijo explicitamente que no hay experiencia relevante.
    refused: bool,
    /// Texto que llego antes de poder decidir. Si al final no se puede ensenar, se tira
    /// sin haber salido nunca a pantalla.
    held: String,
}

impl Gate {
    fn handle(
        &mut self,
        event: ScanEvent,
        fragments: &FragmentSet,
        emit: &mut (dyn FnMut(AnswerEvent) + Send),
    ) {
        match event {
            ScanEvent::Citations(citations) => {
                let verdict = verify::check_citations(&citations, fragments);
                self.citations_ok = Some(matches!(verdict, Verdict::Supported { .. }));
                self.release(emit);
            }
            ScanEvent::Answerable(false) => {
                self.refused = true;
                self.held.clear();
            }
            ScanEvent::Answerable(true) => self.release(emit),
            ScanEvent::AnswerDelta(text) => {
                if self.is_open() {
                    emit(AnswerEvent::Delta { text });
                } else if !self.refused {
                    self.held.push_str(&text);
                }
            }
            ScanEvent::AnswerEnd => {}
        }
    }

    fn is_open(&self) -> bool {
        self.citations_ok == Some(true) && !self.refused
    }

    /// Suelta de golpe lo retenido, la primera vez que se sabe que vale.
    fn release(&mut self, emit: &mut (dyn FnMut(AnswerEvent) + Send)) {
        if self.is_open() && !self.held.is_empty() {
            emit(AnswerEvent::Delta {
                text: std::mem::take(&mut self.held),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::answer::RawCitation;
    use crate::rag::retriever::RetrievedChunk;
    use crate::storage::{DocumentKind, StoredChunk};

    const TEXTO: &str = "Lideré la migración de un monolito a microservicios en Acme.";

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

    /// Ejecuta la compuerta sobre una respuesta completa, troceada como llegaria del
    /// stream, y devuelve el texto que habria llegado a pantalla.
    fn texto_mostrado(json: &str) -> String {
        let fragments = fragmentos();
        let mut gate = Gate::default();
        let mut mostrado = String::new();
        let mut emit = |event: AnswerEvent| {
            if let AnswerEvent::Delta { text } = event {
                mostrado.push_str(&text);
            }
        };

        let chars: Vec<char> = json.chars().collect();
        for chunk in chars.chunks(6) {
            let piece: String = chunk.iter().collect();
            for event in gate.scanner.push(&piece) {
                gate.handle(event, &fragments, &mut emit);
            }
        }

        mostrado
    }

    #[test]
    fn una_respuesta_bien_citada_llega_entera_a_pantalla() {
        let json = r#"{"citations":[{"fragment":1,"quote":"migración de un monolito"}],
                       "answerable":true,
                       "answer":"Lideré una migración a microservicios."}"#;
        assert_eq!(texto_mostrado(json), "Lideré una migración a microservicios.");
    }

    /// El caso que justifica toda la compuerta: el modelo se inventa la experiencia. Ni
    /// una palabra puede llegar a pantalla, ni siquiera para retirarla despues.
    #[test]
    fn una_respuesta_inventada_no_ensena_ni_una_palabra() {
        let json = r#"{"citations":[{"fragment":1,"quote":"dirigí un equipo de ventas"}],
                       "answerable":true,
                       "answer":"Dirigí un equipo de ventas de 20 personas."}"#;
        assert_eq!(texto_mostrado(json), "");
    }

    #[test]
    fn un_fragmento_inventado_cierra_la_compuerta() {
        let json = r#"{"citations":[{"fragment":9,"quote":"lo que sea"}],
                       "answerable":true,
                       "answer":"Algo que no se puede ensenar."}"#;
        assert_eq!(texto_mostrado(json), "");
    }

    #[test]
    fn si_el_modelo_dice_que_no_hay_experiencia_no_se_ensena_su_respuesta() {
        let json = r#"{"citations":[],"answerable":false,
                       "answer":"Podrias contar que aprendiste Rust."}"#;
        assert_eq!(texto_mostrado(json), "");
    }

    /// Si el modelo escribe la respuesta antes que las citas, el texto se retiene y se
    /// suelta de golpe cuando las citas llegan y valen. El orden del prompt es una
    /// optimizacion, no un requisito para que esto sea correcto.
    #[test]
    fn el_texto_adelantado_se_retiene_y_se_suelta_al_verificar() {
        let json = r#"{"answer":"Lideré una migración a microservicios.",
                       "citations":[{"fragment":1,"quote":"migración de un monolito"}],
                       "answerable":true}"#;
        assert_eq!(texto_mostrado(json), "Lideré una migración a microservicios.");
    }

    /// El mismo caso pero con citas invalidas: lo retenido se tira sin ensenarse.
    #[test]
    fn el_texto_adelantado_se_tira_si_las_citas_no_valen() {
        let json = r#"{"answer":"Dirigí un equipo de ventas.",
                       "citations":[{"fragment":1,"quote":"un equipo de ventas"}],
                       "answerable":true}"#;
        assert_eq!(texto_mostrado(json), "");
    }

    #[test]
    fn la_vista_previa_de_un_fragmento_no_vuelca_el_documento_entero() {
        let largo = "palabra ".repeat(100);
        let preview = preview_of(&largo);
        assert!(preview.chars().count() <= 121);
        assert!(preview.ends_with('…'));
    }

    #[test]
    fn un_fragmento_corto_se_ensena_completo_y_sin_puntos_suspensivos() {
        assert_eq!(preview_of("  Ingeniero de software  "), "Ingeniero de software");
    }

    /// Comprobacion de que la compuerta y el veredicto final coinciden: lo que se ensena
    /// en streaming es exactamente lo que sobrevive a la verificacion completa.
    #[test]
    fn la_compuerta_y_el_veredicto_final_dicen_lo_mismo() {
        let casos = [
            (
                r#"{"citations":[{"fragment":1,"quote":"migración de un monolito"}],"answerable":true,"answer":"Ok."}"#,
                true,
            ),
            (
                r#"{"citations":[{"fragment":1,"quote":"inventado"}],"answerable":true,"answer":"No."}"#,
                false,
            ),
            (
                r#"{"citations":[{"fragment":4,"quote":"inventado"}],"answerable":true,"answer":"No."}"#,
                false,
            ),
            (r#"{"citations":[],"answerable":false,"answer":"No."}"#, false),
        ];

        for (json, deberia_valer) in casos {
            let parsed = answer::parse(json).expect("parsear");
            let final_ok = matches!(
                verify::verify(&parsed, &fragmentos()),
                Verdict::Supported { .. }
            );
            let streaming_ok = !texto_mostrado(json).is_empty();

            assert_eq!(
                final_ok, deberia_valer,
                "veredicto final equivocado para {json}"
            );
            assert_eq!(
                streaming_ok, deberia_valer,
                "la compuerta no coincide con el veredicto para {json}"
            );
        }
    }

    /// Una cita vacia no abre la compuerta aunque el numero de fragmento exista.
    #[test]
    fn un_numero_sin_cita_literal_no_abre_la_compuerta() {
        let json = r#"{"citations":[1],"answerable":true,"answer":"Algo."}"#;
        assert_eq!(texto_mostrado(json), "");
        let _ = RawCitation {
            fragment: 1,
            quote: String::new(),
        };
    }
}
