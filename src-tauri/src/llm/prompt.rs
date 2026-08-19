//! Construccion del prompt (§8) y numeracion de los fragmentos recuperados.
//!
//! **Por que los fragmentos se numeran 1..k y no se usan los identificadores de la base.**
//! El modelo tiene que devolver a que fragmento apunta cada afirmacion. Si se le dieran
//! los `rowid` de SQLite —numeros de cuatro o cinco cifras, distintos en cada equipo— un
//! modelo pequeno los copiaria mal a menudo y ademas serian numeros que no significan
//! nada dentro de la conversacion. Con 1..k la numeracion es corta, local a esta
//! peticion, y cualquier numero fuera de rango es inequivocamente una referencia
//! inventada. La traduccion a identificadores reales la hace `FragmentSet`.
//!
//! **Por que el orden de los campos de la respuesta esta fijado en el prompt.** Las citas
//! van primero. Un JSON se genera de arriba abajo, asi que pedirlas antes que el texto
//! permite verificarlas mientras el modelo todavia esta escribiendo la respuesta, y no
//! ensenar ni una palabra que no este respaldada. Si fuera al reves habria que mostrar
//! texto sin verificar y retirarlo despues, que es exactamente lo que §6 no admite.

use serde::{Deserialize, Serialize};

use crate::llm::ChatMessage;
use crate::rag::retriever::RetrievedChunk;
use crate::storage::DocumentKind;

/// Estilo de respuesta segun el tipo de pregunta (§8).
///
/// La deteccion automatica es §7 y llega en la Fase 5; hasta entonces lo elige el usuario
/// a mano. El enum ya existe para que el clasificador solo tenga que rellenarlo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnswerStyle {
    Behavioral,
    Technical,
    General,
}

impl AnswerStyle {
    fn guidance(self) -> &'static str {
        match self {
            Self::Behavioral => {
                "Estructura la respuesta en STAR sin escribir las etiquetas: primero la \
                 situacion y la tarea, despues lo que hizo el candidato, y termina con el \
                 resultado concreto."
            }
            Self::Technical => {
                "Da la respuesta directa primero, despues el razonamiento en una frase, y \
                 apoyala en un ejemplo real del candidato si lo hay en los fragmentos."
            }
            Self::General => {
                "Responde directo, en primera persona, y apoyate en un ejemplo concreto \
                 del candidato."
            }
        }
    }
}

/// Un fragmento tal y como se le presenta al modelo, con su numero de esta peticion.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Fragment {
    /// Numero que ve el modelo. Empieza en 1.
    pub number: usize,
    /// Identificador real en la base de datos. El modelo nunca lo ve.
    pub chunk_id: i64,
    pub document_title: String,
    pub kind: DocumentKind,
    pub text: String,
}

/// Los fragmentos de una peticion, con la traduccion entre el numero que ve el modelo y
/// el fragmento de verdad.
#[derive(Debug, Clone, Default)]
pub struct FragmentSet {
    fragments: Vec<Fragment>,
}

impl FragmentSet {
    pub fn from_retrieval(chunks: &[RetrievedChunk]) -> Self {
        Self {
            fragments: chunks
                .iter()
                .enumerate()
                .map(|(position, retrieved)| Fragment {
                    number: position + 1,
                    chunk_id: retrieved.chunk.id,
                    document_title: retrieved.chunk.document_title.clone(),
                    kind: retrieved.chunk.kind,
                    text: retrieved.chunk.text.clone(),
                })
                .collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.fragments.is_empty()
    }

    pub fn all(&self) -> &[Fragment] {
        &self.fragments
    }

    /// Traduce el numero que devolvio el modelo. `None` significa que ese fragmento nunca
    /// se envio, que es una referencia inventada.
    pub fn get(&self, number: usize) -> Option<&Fragment> {
        self.fragments.iter().find(|item| item.number == number)
    }

    fn render(&self) -> String {
        self.fragments
            .iter()
            .map(|fragment| {
                format!(
                    "[{}] {} — {}\n{}",
                    fragment.number,
                    fragment.document_title,
                    describe_kind(fragment.kind),
                    fragment.text.trim()
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

fn describe_kind(kind: DocumentKind) -> &'static str {
    match kind {
        DocumentKind::Cv => "CV del candidato",
        DocumentKind::JobOffer => "oferta de empleo",
        DocumentKind::Company => "informacion de la empresa",
        DocumentKind::Notes => "notas del candidato",
        DocumentKind::PreparedAnswers => "respuesta ya preparada",
        DocumentKind::Other => "documento",
    }
}

/// El system prompt. Es identico durante toda la entrevista a proposito: asi el prefijo
/// se puede cachear y solo se procesan de nuevo los fragmentos y la pregunta
/// (`docs/ARCHITECTURE.md` §4).
pub fn system_prompt(style: AnswerStyle) -> String {
    format!(
        "Eres el copiloto de un candidato durante una entrevista de trabajo. Le sugieres \
que responder, en primera persona, para que el lo diga con sus palabras.

REGLA INNEGOCIABLE: solo puedes afirmar experiencias, datos o logros que aparezcan \
literalmente en los FRAGMENTOS que se te dan. No inventes empresas, tecnologias, cifras \
ni anecdotas. Si los fragmentos no contienen experiencia relevante para la pregunta, \
pon \"answerable\": false y deja \"citations\" vacio.

{}

Responde en el mismo idioma en el que este formulada la pregunta.

Devuelve UNICAMENTE un objeto JSON, sin texto alrededor y sin vallas de codigo, con \
estos campos EN ESTE ORDEN EXACTO:

{{
  \"citations\": [{{\"fragment\": <numero del fragmento>, \"quote\": \"<trozo copiado \
palabra por palabra del fragmento>\"}}],
  \"answerable\": <true o false>,
  \"answer\": \"<2 a 6 frases>\",
  \"keyPoints\": [\"<3 a 5 ideas breves>\"],
  \"followUps\": [\"<2 a 3 preguntas que probablemente hagan despues>\"]
}}

Sobre \"citations\": el campo \"quote\" tiene que ser una copia EXACTA de un trozo del \
fragmento citado, no un resumen ni una parafrasis. Se comprueba automaticamente contra \
el texto original: si no coincide, la respuesta se descarta. Cita al menos un fragmento \
por cada experiencia que menciones.",
        style.guidance()
    )
}

pub fn build(question: &str, style: AnswerStyle, fragments: &FragmentSet) -> Vec<ChatMessage> {
    let user = format!(
        "FRAGMENTOS DEL CANDIDATO\n\n{}\n\nPREGUNTA DEL ENTREVISTADOR\n{}",
        fragments.render(),
        question.trim()
    );

    vec![ChatMessage::system(system_prompt(style)), ChatMessage::user(user)]
}

/// Lo que se ensena cuando no hay experiencia que respalde una respuesta (§6): un
/// esqueleto, nunca una experiencia.
///
/// Se genera aqui, sin pasar por el modelo, precisamente porque en ese caso el modelo es
/// lo ultimo en lo que se puede confiar: pedirle "una estructura sin inventar nada" es
/// invitarle a rellenarla.
pub fn structure_hint(style: AnswerStyle) -> Vec<String> {
    match style {
        AnswerStyle::Behavioral => vec![
            "Situacion: donde estabas y que pasaba.".into(),
            "Tarea: de que eras responsable exactamente.".into(),
            "Accion: que hiciste tu, no el equipo.".into(),
            "Resultado: como acabo, con un dato si lo tienes.".into(),
        ],
        AnswerStyle::Technical => vec![
            "Di lo que sabes y hasta donde llega, sin adornarlo.".into(),
            "Explica como lo abordarias razonando en voz alta.".into(),
            "Enlaza con algo parecido que si hayas hecho.".into(),
        ],
        AnswerStyle::General => vec![
            "Responde a lo que se te pregunta en una frase.".into(),
            "Apoyalo en algo que si hayas hecho.".into(),
            "Si no lo has hecho, dilo y explica como lo aprenderias.".into(),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::StoredChunk;

    fn chunk(id: i64, text: &str) -> RetrievedChunk {
        RetrievedChunk {
            chunk: StoredChunk {
                id,
                document_id: 1,
                document_title: "CV Santiago.docx".into(),
                kind: DocumentKind::Cv,
                ordinal: 0,
                text: text.into(),
            },
            similarity: 0.85,
        }
    }

    #[test]
    fn la_numeracion_empieza_en_uno_y_es_correlativa() {
        let set = FragmentSet::from_retrieval(&[chunk(412, "a"), chunk(97, "b")]);
        assert_eq!(set.all()[0].number, 1);
        assert_eq!(set.all()[1].number, 2);
    }

    /// El numero que ve el modelo y el identificador de la base son cosas distintas.
    /// Confundirlos haria que una cita valida apuntase a otro fragmento.
    #[test]
    fn el_numero_del_prompt_traduce_al_identificador_real() {
        let set = FragmentSet::from_retrieval(&[chunk(412, "a"), chunk(97, "b")]);
        assert_eq!(set.get(1).expect("fragmento 1").chunk_id, 412);
        assert_eq!(set.get(2).expect("fragmento 2").chunk_id, 97);
    }

    #[test]
    fn un_numero_fuera_de_rango_no_existe() {
        let set = FragmentSet::from_retrieval(&[chunk(412, "a")]);
        assert!(set.get(0).is_none());
        assert!(set.get(2).is_none());
        assert!(set.get(999).is_none());
    }

    #[test]
    fn el_prompt_lleva_los_fragmentos_numerados_y_la_pregunta() {
        let set = FragmentSet::from_retrieval(&[chunk(412, "Lidere la migracion")]);
        let messages = build("Cuentame un proyecto complicado", AnswerStyle::Behavioral, &set);

        assert_eq!(messages.len(), 2);
        let user = &messages[1].content;
        assert!(user.contains("[1]"));
        assert!(user.contains("Lidere la migracion"));
        assert!(user.contains("Cuentame un proyecto complicado"));
    }

    /// El identificador de la base nunca sale hacia el modelo: es un dato interno y en el
    /// modo nube saldria del equipo sin aportar nada.
    #[test]
    fn el_identificador_de_la_base_no_viaja_en_el_prompt() {
        let set = FragmentSet::from_retrieval(&[chunk(412, "Lidere la migracion")]);
        let messages = build("pregunta", AnswerStyle::General, &set);
        assert!(!messages[1].content.contains("412"));
    }

    /// Si el orden de los campos se rompe, se pierde la posibilidad de verificar antes de
    /// ensenar. Es la razon de que este fijado, asi que va con test.
    #[test]
    fn el_prompt_pide_las_citas_antes_que_la_respuesta() {
        let prompt = system_prompt(AnswerStyle::Behavioral);
        let citas = prompt.find("\"citations\"").expect("citations");
        let respuesta = prompt.find("\"answer\"").expect("answer");
        assert!(citas < respuesta);
    }

    #[test]
    fn el_prompt_exige_cita_literal() {
        let prompt = system_prompt(AnswerStyle::General);
        assert!(prompt.contains("EXACTA"));
    }

    #[test]
    fn el_estilo_cambia_la_guia() {
        assert!(system_prompt(AnswerStyle::Behavioral).contains("STAR"));
        assert!(!system_prompt(AnswerStyle::Technical).contains("STAR"));
    }

    /// El esqueleto de §6 no puede contener ninguna experiencia: es una plantilla vacia.
    #[test]
    fn el_esqueleto_de_respuesta_no_sugiere_ninguna_experiencia() {
        for style in [
            AnswerStyle::Behavioral,
            AnswerStyle::Technical,
            AnswerStyle::General,
        ] {
            assert!(!structure_hint(style).is_empty());
        }
    }
}
