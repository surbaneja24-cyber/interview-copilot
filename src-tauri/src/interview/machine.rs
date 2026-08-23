//! La maquina de estados de la entrevista (§7 y §10).
//!
//! Es la pieza que convierte "hay audio entrando por dos sitios" en "ahora toca sugerir".
//! Esta escrita como `TurnDetector` y por el mismo motivo: **sin nada de fuera dentro**. No
//! toca el audio, ni la base, ni el modelo, ni el reloj. Entran eventos, salen ordenes, y
//! todo lo que puede equivocarse en silencio se prueba con eventos escritos a mano.
//!
//! Quien la usa se encarga de obedecer las ordenes; ella no sabe si se han cumplido hasta
//! que se lo dicen con otro evento.
//!
//! ## Quien habla se sabe por la fuente, no por la voz
//!
//! Viene decidido de §4.1: el microfono es el candidato y el loopback es el entrevistador.
//! Aqui eso ya llega resuelto, y por eso los eventos del uno y del otro son distintos en vez
//! de llevar un campo "quien". Un evento con un campo que hay que mirar siempre es una
//! condicion que alguien se va a olvidar.
//!
//! ## Las dos reglas que no son evidentes
//!
//! **1. Los turnos del entrevistador se acumulan hasta que el candidato conteste.**
//!
//! Nadie pregunta en una sola frase. "Cuentame un proyecto complicado." — pausa — "Y ponme
//! un ejemplo con cifras si las tienes." Son dos turnos del VAD y **una** pregunta, y
//! contestar a la primera mitad es contestar a otra cosa. Mientras el candidato no haya
//! hablado, cada turno nuevo del entrevistador se pega al anterior y la preparacion se
//! rehace desde el principio.
//!
//! Rehacerla es barato y esta contado en §4: la recuperacion cuesta milisegundos y se lanza
//! de todas formas en cuanto hay una pausa. Lo caro es lo otro — dejar en pantalla la
//! respuesta a media pregunta mientras el entrevistador termina de formularla.
//!
//! La frontera no es un temporizador: **es que el candidato hable**. En cuanto contesta, el
//! siguiente turno del entrevistador es una pregunta nueva. Un temporizador habria hecho
//! falta calibrarlo, y esto no.
//!
//! **2. Que el candidato empiece a hablar no tira la sugerencia.**
//!
//! Al reves de lo que pide el instinto. El candidato empieza a contestar de memoria mientras
//! la sugerencia todavia se esta preparando —es lo normal, tarda unos segundos— y quitarsela
//! justo cuando aparece seria dejarle solo en el unico momento en que la queria. La
//! sugerencia se queda en pantalla hasta la pregunta siguiente.

use super::trigger::{self, Skip};

/// En que punto de la entrevista estamos.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum State {
    /// Fuera de entrevista.
    Off,
    /// Dentro, esperando a que el entrevistador diga algo.
    Waiting,
    /// El entrevistador esta hablando ahora mismo.
    Asking,
    /// Turno cerrado y sugerencia en preparacion.
    Preparing { question: String },
    /// Hay una sugerencia en pantalla.
    Suggesting { question: String },
    /// El candidato esta contestando, con la sugerencia todavia delante.
    Answering { question: String },
}

/// Lo que pasa fuera.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Enter,
    Leave,
    /// El loopback ha detectado voz: el entrevistador arranca.
    InterviewerStarted,
    /// Turno del entrevistador cerrado y transcrito.
    InterviewerSaid { text: String },
    /// El microfono ha detectado voz: el candidato arranca.
    CandidateStarted,
    /// La sugerencia esta lista.
    Suggested,
    /// No se pudo preparar. Da igual por que: la maquina solo necesita saber que no llega.
    SuggestionFailed,
}

/// Lo que hay que hacer. Quien usa la maquina lo obedece; ella no lo comprueba.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Nothing,
    /// Preparar una sugerencia para esta pregunta. Si habia una preparacion en marcha,
    /// **se abandona**: esta la sustituye.
    Prepare { question: String },
    /// Abandonar lo que se estuviera preparando, sin empezar nada.
    Abandon,
}

/// La maquina.
#[derive(Debug)]
pub struct Interview {
    state: State,
    /// Turnos del entrevistador desde la ultima vez que hablo el candidato.
    ///
    /// No se guarda solo el ultimo: la pregunta puede venir en varios turnos y lo que se
    /// manda a preparar es la pregunta entera.
    turns: Vec<String>,
    /// Turnos que se han descartado por no parecer preguntas. Se cuentan porque lo que se
    /// tira hay que poder verlo: si esto crece rapido, el filtro esta comiendose preguntas.
    skipped: usize,
}

impl Default for Interview {
    fn default() -> Self {
        Self::new()
    }
}

impl Interview {
    pub fn new() -> Self {
        Self {
            state: State::Off,
            turns: Vec::new(),
            skipped: 0,
        }
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    pub fn skipped(&self) -> usize {
        self.skipped
    }

    /// La pregunta acumulada hasta ahora, tal y como se mandaria a preparar.
    fn question(&self) -> String {
        self.turns.join(" ")
    }

    pub fn push(&mut self, event: Event) -> Action {
        match event {
            Event::Leave => {
                let habia_trabajo = matches!(self.state, State::Preparing { .. });
                self.state = State::Off;
                self.turns.clear();
                return if habia_trabajo { Action::Abandon } else { Action::Nothing };
            }
            Event::Enter => {
                self.state = State::Waiting;
                self.turns.clear();
                return Action::Nothing;
            }
            _ if self.state == State::Off => {
                // Fuera de entrevista no se escucha nada. Que llegue un turno aqui no es un
                // error: la captura puede seguir abierta para el medidor de Ajustes.
                return Action::Nothing;
            }
            _ => {}
        }

        match event {
            Event::InterviewerStarted => {
                // Solo cambia la pantalla. No se abandona lo que se este preparando: si el
                // entrevistador esta ampliando la pregunta, el turno que cierre lo dira, y
                // si era un carraspeo no habra pasado nada.
                self.state = State::Asking;
                Action::Nothing
            }

            Event::InterviewerSaid { text } => {
                if let Err(motivo) = trigger::should_prepare(&text, !self.turns.is_empty()) {
                    self.skipped += 1;
                    // Vuelve a esperar sin tocar lo acumulado: si el entrevistador dijo
                    // "vale" en mitad de una pregunta en dos partes, la primera parte sigue
                    // valiendo.
                    self.state = match self.turns.is_empty() {
                        true => State::Waiting,
                        false => State::Suggesting { question: self.question() },
                    };
                    let _ = motivo;
                    return Action::Nothing;
                }

                // Se pega a lo anterior mientras el candidato no haya contestado.
                self.turns.push(text);
                let question = self.question();
                self.state = State::Preparing { question: question.clone() };
                Action::Prepare { question }
            }

            Event::CandidateStarted => {
                // Contestar cierra la pregunta: lo siguiente que diga el entrevistador ya es
                // otra cosa. Y la sugerencia se queda donde esta.
                let question = self.question();
                self.turns.clear();
                self.state = match self.state {
                    // Todavia preparandola: que conteste no la cancela. Puede estar
                    // contestando de memoria y quererla igual dentro de dos segundos.
                    State::Preparing { .. } => State::Preparing { question },
                    _ => State::Answering { question },
                };
                Action::Nothing
            }

            Event::Suggested => {
                if let State::Preparing { question } = &self.state {
                    self.state = State::Suggesting { question: question.clone() };
                }
                Action::Nothing
            }

            Event::SuggestionFailed => {
                if let State::Preparing { question } = &self.state {
                    // Se queda como sugerencia igual: la pantalla tiene que poder decir "no
                    // he podido con esta". Volver a `Waiting` borraria la pregunta y con
                    // ella cualquier rastro de que se intento.
                    self.state = State::Suggesting { question: question.clone() };
                }
                Action::Nothing
            }

            Event::Enter | Event::Leave => unreachable!("resueltos arriba"),
        }
    }
}

/// Existe para que quien lea el `Skip` sepa que no se esta ignorando.
impl From<Skip> for &'static str {
    fn from(skip: Skip) -> Self {
        match skip {
            Skip::TooShort => "demasiado corto",
            Skip::NotAQuestion => "no parece una pregunta",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PREGUNTA: &str = "Cuéntame un proyecto complicado en el que hayas trabajado";
    const AMPLIACION: &str = "Y ponme un ejemplo con cifras si las tienes";
    /// Una pregunta nueva de verdad. No vale reutilizar `AMPLIACION`: sola no pasa por
    /// pregunta, que es justo lo que dice `trigger`, y usarla aqui midio dos veces el mismo
    /// filtro en vez de la maquina.
    const OTRA: &str = "¿Por qué quieres trabajar aquí?";

    fn en_marcha() -> Interview {
        let mut interview = Interview::new();
        interview.push(Event::Enter);
        interview
    }

    #[test]
    fn fuera_de_entrevista_no_pasa_nada() {
        let mut interview = Interview::new();
        let accion = interview.push(Event::InterviewerSaid { text: PREGUNTA.into() });
        assert_eq!(accion, Action::Nothing);
        assert_eq!(*interview.state(), State::Off);
    }

    #[test]
    fn una_pregunta_manda_prepararla() {
        let mut interview = en_marcha();
        interview.push(Event::InterviewerStarted);
        assert_eq!(*interview.state(), State::Asking);

        let accion = interview.push(Event::InterviewerSaid { text: PREGUNTA.into() });
        assert_eq!(accion, Action::Prepare { question: PREGUNTA.into() });
    }

    /// La regla que no es evidente: **una pregunta en dos frases es una pregunta.**
    /// Contestar a la primera mitad es contestar a otra cosa.
    #[test]
    fn dos_turnos_seguidos_del_entrevistador_son_una_sola_pregunta() {
        let mut interview = en_marcha();
        interview.push(Event::InterviewerSaid { text: PREGUNTA.into() });
        let accion = interview.push(Event::InterviewerSaid { text: AMPLIACION.into() });

        match accion {
            Action::Prepare { question } => {
                assert!(question.starts_with(PREGUNTA), "{question}");
                assert!(question.ends_with(AMPLIACION), "{question}");
            }
            otra => panic!("se esperaba rehacer la preparacion y salio {otra:?}"),
        }
    }

    /// Y la frontera no es un temporizador: es que el candidato conteste.
    #[test]
    fn despues_de_contestar_el_entrevistador_empieza_una_pregunta_nueva() {
        let mut interview = en_marcha();
        interview.push(Event::InterviewerSaid { text: PREGUNTA.into() });
        interview.push(Event::Suggested);
        interview.push(Event::CandidateStarted);

        let accion = interview.push(Event::InterviewerSaid { text: OTRA.into() });
        assert_eq!(
            accion,
            Action::Prepare { question: OTRA.into() },
            "la pregunta anterior se ha colado en la nueva"
        );
    }

    /// La otra regla que va contra el instinto: que el candidato empiece a hablar **no**
    /// cancela la sugerencia que se esta preparando.
    #[test]
    fn contestar_de_memoria_no_cancela_la_sugerencia() {
        let mut interview = en_marcha();
        interview.push(Event::InterviewerSaid { text: PREGUNTA.into() });

        let accion = interview.push(Event::CandidateStarted);
        assert_eq!(accion, Action::Nothing, "se ha abandonado la preparacion");
        assert!(
            matches!(interview.state(), State::Preparing { .. }),
            "salio {:?}",
            interview.state()
        );

        interview.push(Event::Suggested);
        assert!(matches!(interview.state(), State::Suggesting { .. }));
    }

    #[test]
    fn un_vale_no_dispara_una_sugerencia() {
        let mut interview = en_marcha();
        let accion = interview.push(Event::InterviewerSaid { text: "Vale, perfecto".into() });

        assert_eq!(accion, Action::Nothing);
        assert_eq!(interview.skipped(), 1, "lo que se tira tiene que contarse");
        assert_eq!(*interview.state(), State::Waiting);
    }

    /// Y un "vale" en mitad de una pregunta en dos partes no se lleva por delante la primera.
    #[test]
    fn un_vale_a_media_pregunta_no_borra_lo_que_ya_habia() {
        let mut interview = en_marcha();
        interview.push(Event::InterviewerSaid { text: PREGUNTA.into() });
        interview.push(Event::InterviewerSaid { text: "Vale".into() });

        let accion = interview.push(Event::InterviewerSaid { text: AMPLIACION.into() });
        match accion {
            Action::Prepare { question } => assert!(question.starts_with(PREGUNTA), "{question}"),
            otra => panic!("se perdio la primera mitad: {otra:?}"),
        }
    }

    #[test]
    fn si_falla_la_sugerencia_la_pregunta_no_se_pierde() {
        let mut interview = en_marcha();
        interview.push(Event::InterviewerSaid { text: PREGUNTA.into() });
        interview.push(Event::SuggestionFailed);

        match interview.state() {
            State::Suggesting { question } => assert_eq!(question, PREGUNTA),
            otro => panic!("salio {otro:?}"),
        }
    }

    #[test]
    fn salir_abandona_lo_que_estuviera_en_marcha() {
        let mut interview = en_marcha();
        interview.push(Event::InterviewerSaid { text: PREGUNTA.into() });

        assert_eq!(interview.push(Event::Leave), Action::Abandon);
        assert_eq!(*interview.state(), State::Off);

        // Y salir sin nada en marcha no manda abandonar nada.
        let mut tranquila = en_marcha();
        assert_eq!(tranquila.push(Event::Leave), Action::Nothing);
    }

    /// Entrar dos veces no arrastra la entrevista anterior.
    #[test]
    fn entrar_de_nuevo_empieza_de_cero() {
        let mut interview = en_marcha();
        interview.push(Event::InterviewerSaid { text: PREGUNTA.into() });
        interview.push(Event::Enter);

        assert_eq!(*interview.state(), State::Waiting);
        let accion = interview.push(Event::InterviewerSaid { text: OTRA.into() });
        assert_eq!(accion, Action::Prepare { question: OTRA.into() });
    }
}
