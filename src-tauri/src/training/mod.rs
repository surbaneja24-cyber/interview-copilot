//! El banco de preguntas de entrenamiento (§5 y §12).
//!
//! La regla de §6 —la IA no inventa experiencia— no se sostiene con un filtro despues de
//! generar, y eso esta medido: la cita literal demuestra que el modelo leyo los documentos,
//! no que la respuesta salga de ellos (`ARCHITECTURE.md` §5). Se sostiene teniendo material
//! de verdad en el momento de la pregunta, y el material lo pone el candidato antes.
//!
//! De ahi este banco. Cada respuesta que el usuario da aqui se indexa como cualquier otro
//! documento, **sin proyecto**, asi que vale para esta entrevista y para las siguientes.
//! Dos efectos que no son evidentes:
//!
//! - **La cita literal deja de estorbar.** Citar un CV telegrafico escrito en tercera
//!   persona es dificil; citar la respuesta que tu mismo diste, no.
//! - **La pregunta se parece a la pregunta.** Lo que se guarda es "Pregunta: … Respuesta: …",
//!   asi que en la entrevista el parecido se mide entre preguntas, que es donde de verdad
//!   funciona la recuperacion asimetrica para la que esta entrenado E5 (§2.1).
//!
//! Las preguntas de aqui son fijas y estan escritas a mano. Generarlas con el LLM a partir
//! de la oferta llega despues: un banco fijo funciona sin modelo, sin red y sin latencia, y
//! es lo que permite entrenar en un portatil que no mueve un 3B.

pub mod classifier;
#[cfg(test)]
mod corpus;
pub mod review;

use serde::Serialize;

/// El tipo de pregunta, de la taxonomia de §7. Se guarda con la respuesta para poder
/// entrenar por temas y, mas adelante, detectar de que tipo flojea el candidato (§13).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum QuestionKind {
    /// "Cuentame una vez que…". Piden una historia concreta.
    Behavioral,
    /// Por que esta empresa, este puesto, este cambio.
    Motivation,
    /// Lo que sabe hacer y con que lo ha hecho.
    Experience,
    /// "Que harias si…". No piden pasado, piden criterio.
    Situational,
    /// Fortalezas, debilidades, fracasos.
    SelfAssessment,
    /// Dinero, disponibilidad, condiciones.
    Logistics,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrainingQuestion {
    pub id: &'static str,
    pub kind: QuestionKind,
    pub text: &'static str,
    /// Que tiene que llevar dentro una buena respuesta. No es decoracion: una respuesta
    /// entrenada sin cifras ni resultado es la que luego obliga al modelo a rellenar los
    /// huecos, que es exactamente lo que no puede hacer.
    pub hint: &'static str,
}

/// El banco. Corto a proposito: veinte preguntas que alguien contesta en una tarde valen
/// mas que cien que no contesta nadie.
pub const QUESTIONS: &[TrainingQuestion] = &[
    TrainingQuestion {
        id: "presentate",
        kind: QuestionKind::Experience,
        text: "Cuéntame un poco sobre ti",
        hint: "Dos minutos: qué haces ahora, qué has hecho antes y por qué estás buscando. \
               Sin repetir el CV línea por línea.",
    },
    TrainingQuestion {
        id: "proyecto-complicado",
        kind: QuestionKind::Behavioral,
        text: "Cuéntame un proyecto complicado en el que hayas trabajado",
        hint: "Situación, qué te tocaba a ti, qué hiciste y cómo acabó. Con cifras si las \
               tienes: cuánta gente, cuánto tiempo, cuánto mejoró.",
    },
    TrainingQuestion {
        id: "conflicto",
        kind: QuestionKind::Behavioral,
        text: "Háblame de una vez que tuviste un conflicto con un compañero",
        hint: "Qué pasó, qué hiciste tú —no qué hizo el otro— y en qué quedó. El final \
               importa más que el conflicto.",
    },
    TrainingQuestion {
        id: "error",
        kind: QuestionKind::SelfAssessment,
        text: "Cuéntame un error que hayas cometido y qué aprendiste",
        hint: "Un error de verdad, con consecuencia real, y qué cambiaste después para que \
               no volviera a pasar.",
    },
    TrainingQuestion {
        id: "presion",
        kind: QuestionKind::Behavioral,
        text: "¿Cómo trabajas bajo presión? Dame un ejemplo",
        hint: "Un día concreto en el que ibas justo: qué priorizaste y qué dejaste caer.",
    },
    TrainingQuestion {
        id: "liderazgo",
        kind: QuestionKind::Behavioral,
        text: "Cuéntame una vez que tuviste que organizar o dirigir a otros",
        hint: "No hace falta ser jefe: coordinar un turno o enseñar a alguien nuevo cuenta. \
               Cuántas personas y qué salió de ahí.",
    },
    TrainingQuestion {
        id: "por-que-empresa",
        kind: QuestionKind::Motivation,
        text: "¿Por qué quieres trabajar aquí?",
        hint: "Algo concreto de esta empresa y algo concreto de ti. Si vale para cualquier \
               empresa, no vale para esta.",
    },
    TrainingQuestion {
        id: "por-que-tu",
        kind: QuestionKind::Motivation,
        text: "¿Por qué deberíamos contratarte a ti?",
        hint: "Dos o tres cosas que sabes hacer y que el puesto pide, cada una con la prueba \
               de dónde lo has hecho.",
    },
    TrainingQuestion {
        id: "fortalezas",
        kind: QuestionKind::SelfAssessment,
        text: "¿Cuáles dirías que son tus puntos fuertes?",
        hint: "Dos, con un ejemplo cada uno. Sin adjetivos sueltos.",
    },
    TrainingQuestion {
        id: "debilidades",
        kind: QuestionKind::SelfAssessment,
        text: "¿Y tu mayor defecto?",
        hint: "Uno real y qué haces para compensarlo. \"Soy muy perfeccionista\" no cuela.",
    },
    TrainingQuestion {
        id: "cambio",
        kind: QuestionKind::Motivation,
        text: "¿Por qué dejaste tu trabajo anterior, o por qué quieres cambiar?",
        hint: "Hacia dónde vas, no de qué huyes. Sin hablar mal de nadie.",
    },
    TrainingQuestion {
        id: "aprender",
        kind: QuestionKind::Behavioral,
        text: "Cuéntame algo que hayas tenido que aprender por tu cuenta",
        hint: "Qué necesitabas, cómo lo aprendiste y para qué lo usaste después.",
    },
    TrainingQuestion {
        id: "cliente-dificil",
        kind: QuestionKind::Situational,
        text: "¿Qué harías con un cliente o un compañero enfadado?",
        hint: "Si te ha pasado, cuenta ese caso; si no, di cómo lo abordarías y por qué.",
    },
    TrainingQuestion {
        id: "no-se",
        kind: QuestionKind::Situational,
        text: "¿Qué haces cuando no sabes cómo resolver algo?",
        hint: "Los pasos que das de verdad: a quién preguntas, dónde buscas, cuándo escalas.",
    },
    TrainingQuestion {
        id: "logro",
        kind: QuestionKind::Experience,
        text: "¿De qué logro profesional estás más orgulloso?",
        hint: "Qué había antes, qué hiciste y qué quedó después. Con cifras si las hay.",
    },
    TrainingQuestion {
        id: "herramientas",
        kind: QuestionKind::Experience,
        text: "¿Con qué herramientas o programas sueles trabajar?",
        hint: "Cuáles, cuánto tiempo y para qué exactamente. Distinguiendo lo que dominas de \
               lo que has tocado.",
    },
    TrainingQuestion {
        id: "rutina",
        kind: QuestionKind::Experience,
        text: "¿Cómo es un día normal en tu trabajo actual o en el último?",
        hint: "De la mañana a la tarde. Es la pregunta que más material da para las demás.",
    },
    TrainingQuestion {
        id: "expectativas",
        kind: QuestionKind::Logistics,
        text: "¿Qué expectativa salarial tienes?",
        hint: "Una horquilla y en qué la basas. Y qué harías si te preguntan antes de que \
               tú sepas lo que ofrecen.",
    },
    TrainingQuestion {
        id: "disponibilidad",
        kind: QuestionKind::Logistics,
        text: "¿Cuál es tu disponibilidad? ¿Turnos, viajes, mudanza?",
        hint: "Lo que puedes y lo que no. Es mejor decirlo aquí que descubrirlo después.",
    },
    TrainingQuestion {
        id: "preguntas-tuyas",
        kind: QuestionKind::Motivation,
        text: "¿Tienes alguna pregunta para nosotros?",
        hint: "Dos o tres, sobre el equipo, el día a día o cómo se mide que lo estás \
               haciendo bien. Decir que no es la peor respuesta posible.",
    },
];
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn los_identificadores_son_unicos() {
        // Se usan para saber que preguntas estan contestadas: dos iguales darian una
        // respuesta por contestada sin estarlo.
        let ids: HashSet<&str> = QUESTIONS.iter().map(|question| question.id).collect();
        assert_eq!(ids.len(), QUESTIONS.len());
    }

    #[test]
    fn ninguna_pregunta_se_queda_sin_pista() {
        for question in QUESTIONS {
            assert!(!question.text.is_empty(), "{}", question.id);
            assert!(
                question.hint.len() > 20,
                "{} necesita una pista util, no una frase hecha",
                question.id
            );
        }
    }

    /// El banco tiene que cubrir todos los tipos de §7 que se entrenan. Si algun dia se
    /// anade un tipo y nadie escribe preguntas, este test lo dice.
    #[test]
    fn hay_preguntas_de_todos_los_tipos() {
        let tipos: HashSet<QuestionKind> = QUESTIONS.iter().map(|question| question.kind).collect();
        assert_eq!(
            tipos.len(),
            6,
            "faltan tipos de pregunta en el banco: {tipos:?}"
        );
    }
}
