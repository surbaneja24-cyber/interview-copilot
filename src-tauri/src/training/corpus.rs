//! Preguntas de entrevista etiquetadas a mano, para medir el clasificador de §7.
//!
//! **Este fichero se escribio antes que las reglas y no se toca para que salgan mejor.** Es
//! la misma disciplina que dejo Trading Lab: un conjunto de evaluacion que se ajusta hasta
//! que el resultado gusta ha dejado de medir nada y se ha convertido en la propia
//! implementacion escrita dos veces.
//!
//! Hay tres corpus y cada uno contesta a una cosa distinta:
//!
//! - `BANCO` no esta aqui: son las veinte preguntas de `QUESTIONS`, con el tipo que ya
//!   llevaban puesto. Es el corpus de **desarrollo**, el que se mira mientras se escriben las
//!   reglas. Acertar aqui no demuestra gran cosa.
//! - `EVALUACION` son preguntas de entrevista de verdad, dichas como las dice un
//!   entrevistador y no como las escribe el banco. Es el corpus **sellado**: se mira al
//!   final y lo que salga es lo que se publica.
//! - `SIN_TIPO` son preguntas que **no deben clasificarse**. Es el control, y sin el la
//!   arquitectura entera no se sostiene: si el clasificador nunca dice "no se", entonces "va
//!   por reglas y el LLM solo resuelve la ambiguedad" es mentira, porque no hay ambiguedad
//!   que resolver — hay un valor por defecto disfrazado de decision.

#![cfg(test)]

use super::QuestionKind;

/// El corpus sellado. Preguntas reales de entrevista, con el tipo puesto a mano.
///
/// Escritas pensando en como pregunta alguien, no en que patrones hay implementados: por eso
/// hay perifrasis, preguntas partidas en dos frases y vocabulario que no aparece en el banco.
pub const EVALUACION: &[(&str, QuestionKind)] = &[
    // Behavioral: piden una historia concreta que ya paso.
    ("Ponme un ejemplo de un día que se te complicó todo", QuestionKind::Behavioral),
    ("¿Recuerdas alguna ocasión en la que tuvieras que decir que no a un cliente?", QuestionKind::Behavioral),
    ("Descríbeme una situación en la que te tocara aprender algo rápido", QuestionKind::Behavioral),
    ("¿Alguna vez has tenido que corregir a un compañero? ¿Cómo lo hiciste?", QuestionKind::Behavioral),
    ("Cuéntame la última vez que algo salió mal por tu parte", QuestionKind::Behavioral),
    ("Dame un ejemplo de una decisión difícil que hayas tomado en el trabajo", QuestionKind::Behavioral),
    // Situational: hipoteticas, piden criterio.
    ("¿Qué harías si un pedido urgente llega mal etiquetado?", QuestionKind::Situational),
    ("Imagina que tu encargado te pide dos cosas a la vez. ¿Por cuál empiezas?", QuestionKind::Situational),
    ("Si detectaras un fallo el último día antes de una entrega, ¿cómo actuarías?", QuestionKind::Situational),
    ("Supongamos que no estás de acuerdo con una instrucción. ¿Qué haces?", QuestionKind::Situational),
    ("¿Cómo reaccionarías si te cambiaran de turno sin avisar?", QuestionKind::Situational),
    // Motivation: por que nosotros, por que este puesto, por que te vas.
    ("¿Qué te llamó la atención de esta oferta?", QuestionKind::Motivation),
    ("¿Por qué dejaste tu anterior empleo?", QuestionKind::Motivation),
    ("¿Qué sabes de nosotros?", QuestionKind::Motivation),
    ("¿Dónde te ves dentro de cinco años?", QuestionKind::Motivation),
    ("¿Qué esperas encontrar en este puesto que no tuvieras antes?", QuestionKind::Motivation),
    // Experience: que sabe hacer y con que lo ha hecho.
    ("¿Con qué programas de gestión de almacén has trabajado?", QuestionKind::Experience),
    ("¿Cuánto tiempo llevas dedicándote a esto?", QuestionKind::Experience),
    ("Háblame de tu formación", QuestionKind::Experience),
    ("¿En qué consiste tu trabajo actual, un día cualquiera?", QuestionKind::Experience),
    ("¿Qué nivel de inglés tienes?", QuestionKind::Experience),
    ("¿Manejas la carretilla retráctil?", QuestionKind::Experience),
    // SelfAssessment: sobre uno mismo.
    ("¿Cuál dirías que es tu mayor defecto?", QuestionKind::SelfAssessment),
    ("¿En qué crees que tienes que mejorar?", QuestionKind::SelfAssessment),
    ("¿Qué dirían de ti tus antiguos compañeros?", QuestionKind::SelfAssessment),
    ("¿De qué trabajo te sientes más orgulloso?", QuestionKind::SelfAssessment),
    // Logistics: dinero, fechas, condiciones.
    ("¿Cuáles son tus expectativas salariales?", QuestionKind::Logistics),
    ("¿Cuándo podrías incorporarte?", QuestionKind::Logistics),
    ("¿Tienes vehículo propio para llegar al polígono?", QuestionKind::Logistics),
    ("¿Tendrías problema con los turnos rotativos?", QuestionKind::Logistics),
    ("¿Estás en algún otro proceso de selección?", QuestionKind::Logistics),
    ("¿Te importaría trasladarte a otra provincia?", QuestionKind::Logistics),
];

/// **El control.** Lo que se dice en una entrevista y no es una pregunta de las seis clases.
///
/// Tienen que salir sin tipo. Un clasificador que le pone etiqueta a "¿me oyes bien?" no esta
/// clasificando, esta contestando siempre lo mismo con un nombre distinto — y entonces el
/// numero de aciertos de arriba tampoco significa nada.
/// **Corregido una vez, el 2026-08-22, y queda dicho.** Aqui estaba "¿Tienes alguna
/// pregunta para nosotros?", y esa pregunta la tiene el banco de `QUESTIONS` como
/// `Motivation` desde el 2026-08-19. El banco es anterior a este corpus, asi que la
/// equivocada era esta lista, no el banco. Es una correccion de etiqueta contra una
/// autoridad externa y anterior — no un ajuste para que salga mejor un numero, que es lo que
/// destruiria el sello.
pub const SIN_TIPO: &[&str] = &[
    "¿Me oyes bien?",
    "¿Puedes ponerte más cerca del micrófono?",
    "Un momento que llamo a mi compañera",
    "¿Qué tal el viaje hasta aquí?",
    "Vale, perfecto",
    "Entonces te llamamos la semana que viene, ¿te parece?",
    "¿Y eso?",
];
