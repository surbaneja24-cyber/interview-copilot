//! De lo que reporta el audio a los eventos de la maquina.
//!
//! `machine` no sabe nada del mundo: entran eventos, salen ordenes. Alguien tiene que
//! traducir "el transcriptor tiene tres entradas nuevas y el loopback esta dando voz" a esos
//! eventos, y esa traduccion es **justo donde se esconden los fallos silenciosos**: repetir
//! una entrada ya vista, perder un turno, o confundir quien hablaba. Por eso vive aqui, en
//! una pieza propia y con tests, en vez de repartida por el comando que la llama.
//!
//! ## Quien habla se sabe por la fuente
//!
//! Viene de §4.1: el microfono es el candidato y el loopback es el entrevistador. Aqui eso se
//! traduce en que las entradas del transcriptor **se filtran por `Source`** antes de mirarlas.
//! Lo que dice el candidato no es una pregunta, por muy bien transcrito que este.
//!
//! ## Y cuando empieza a hablar se sabe por el VAD, no por el texto
//!
//! Es la distincion que hace util esta pieza. El texto de un turno llega entre 2,4 y 3,8 s
//! despues de que se cierre (§4.7), asi que enterarse por el de que alguien ha empezado a
//! hablar es enterarse tarde — y "el candidato ha empezado a contestar" es el evento que
//! cierra la pregunta en `machine`. Ese llega del VAD, que lo sabe en 64 ms.
//!
//! Asi que las dos fuentes de eventos no son intercambiables y cada una trae lo suyo:
//!
//! | Evento | De donde |
//! |---|---|
//! | `InterviewerStarted` / `CandidateStarted` | el VAD, en cuanto abre turno |
//! | `InterviewerSaid` | el transcriptor, segundos despues |
//!
//! ## La maquina avanza cuando alguien mira
//!
//! `observe` lo llama el comando de estado, al ritmo al que la pantalla pregunte. No hay
//! ningun hilo propio y no hace falta: **nada de la maquina depende del reloj**. Si nadie
//! mira, no es que se pierda nada, es que todavia no se ha calculado. La alternativa —un hilo
//! que sondea— seria un hilo mas por una ventaja que no existe.

use super::machine::{Action, Event, Interview, State};
use crate::audio::Source;
use crate::stt::transcriber::Entry;

/// Lo que se ve del mundo en un momento dado.
pub struct Snapshot<'a> {
    /// Todas las entradas del transcriptor, acumulativas. La sesion se acuerda de cuantas
    /// habia visto.
    pub entries: &'a [Entry],
    /// El VAD del microfono esta dando voz ahora mismo.
    pub mic_speaking: bool,
    /// El VAD del loopback esta dando voz ahora mismo.
    pub system_speaking: bool,
}

/// La entrevista en marcha: la maquina mas lo que hace falta para alimentarla.
#[derive(Debug)]
pub struct Session {
    machine: Interview,
    /// Entradas del transcriptor ya convertidas en eventos.
    ///
    /// Se guarda el numero y no la ultima entrada: comparar por contenido haria que dos
    /// respuestas identicas seguidas —"si", "si"— contaran como una.
    seen: usize,
    mic_speaking: bool,
    system_speaking: bool,
    /// La pregunta que la maquina ha mandado preparar y que nadie ha recogido todavia.
    pending: Option<String>,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    pub fn new() -> Self {
        Self {
            machine: Interview::new(),
            seen: 0,
            mic_speaking: false,
            system_speaking: false,
            pending: None,
        }
    }

    pub fn state(&self) -> &State {
        self.machine.state()
    }

    pub fn skipped(&self) -> usize {
        self.machine.skipped()
    }

    /// Entrar en la entrevista.
    ///
    /// Se pone al dia con lo que ya hubiera transcrito **sin generar eventos**: si la captura
    /// llevaba abierta un rato para ver el medidor, lo dicho antes de entrar no es parte de
    /// la entrevista y contestarlo seria empezar por una pregunta que nadie ha hecho.
    pub fn enter(&mut self, entries: usize) {
        self.machine.push(Event::Enter);
        self.seen = entries;
        self.pending = None;
        self.mic_speaking = false;
        self.system_speaking = false;
    }

    pub fn leave(&mut self) -> Action {
        self.pending = None;
        self.machine.push(Event::Leave)
    }

    /// La pregunta pendiente de preparar, si la hay. Se la lleva quien la recoge.
    pub fn take_pending(&mut self) -> Option<String> {
        self.pending.take()
    }

    /// Mira el mundo y adelanta la maquina hasta ponerse al dia.
    ///
    /// El orden importa y es el de los hechos: primero quien ha empezado a hablar, que lo
    /// sabe el VAD al momento, y despues lo que se dijo, que llega segundos mas tarde. Al
    /// reves, el texto de un turno se procesaria antes que el aviso de que alguien empezo a
    /// hablar despues, y `machine` cerraria la pregunta en el orden equivocado.
    pub fn observe(&mut self, snapshot: &Snapshot) {
        if *self.machine.state() == State::Off {
            // Fuera de entrevista se sigue viendo pasar el transcriptor, para que al entrar
            // no se procese de golpe todo lo dicho antes.
            self.seen = snapshot.entries.len();
            return;
        }

        // Flancos del VAD: solo el paso de callado a hablando es un evento.
        if snapshot.system_speaking && !self.system_speaking {
            self.apply(Event::InterviewerStarted);
        }
        if snapshot.mic_speaking && !self.mic_speaking {
            self.apply(Event::CandidateStarted);
        }
        self.system_speaking = snapshot.system_speaking;
        self.mic_speaking = snapshot.mic_speaking;

        for entry in snapshot.entries.iter().skip(self.seen) {
            // Lo que dice el candidato no es una pregunta, por bien transcrito que este.
            if entry.source == Source::System {
                self.apply(Event::InterviewerSaid { text: entry.text.clone() });
            }
        }
        self.seen = snapshot.entries.len();
    }

    /// Le dice a la maquina que la sugerencia esta lista, o que no va a llegar.
    pub fn suggestion(&mut self, ok: bool) {
        self.apply(if ok { Event::Suggested } else { Event::SuggestionFailed });
    }

    fn apply(&mut self, event: Event) {
        match self.machine.push(event) {
            Action::Prepare { question } => {
                // Sustituye a la que hubiera sin recoger: si el entrevistador ha ampliado la
                // pregunta, la de antes era media pregunta y preparar las dos seria gastar
                // una pasada en la version incompleta.
                self.pending = Some(question);
            }
            Action::Abandon => self.pending = None,
            Action::Nothing => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PREGUNTA: &str = "Cuéntame un proyecto complicado en el que hayas trabajado";

    fn entrada(source: Source, texto: &str) -> Entry {
        Entry {
            source,
            text: texto.to_owned(),
            audio_ms: 3_000,
            took_ms: 3_000,
        }
    }

    fn quieto(entries: &[Entry]) -> Snapshot<'_> {
        Snapshot { entries, mic_speaking: false, system_speaking: false }
    }

    #[test]
    fn una_pregunta_del_loopback_deja_algo_que_preparar() {
        let mut session = Session::new();
        session.enter(0);

        let entries = vec![entrada(Source::System, PREGUNTA)];
        session.observe(&quieto(&entries));

        assert_eq!(session.take_pending().as_deref(), Some(PREGUNTA));
    }

    /// Lo que dice el candidato no dispara nada, por bien transcrito que este.
    #[test]
    fn lo_que_dice_el_candidato_no_es_una_pregunta() {
        let mut session = Session::new();
        session.enter(0);

        let entries = vec![entrada(Source::Mic, PREGUNTA)];
        session.observe(&quieto(&entries));

        assert_eq!(session.take_pending(), None);
    }

    /// El fallo mas facil de cometer aqui: volver a mandar lo mismo en cada sondeo. La
    /// pantalla pregunta varias veces por segundo.
    #[test]
    fn mirar_dos_veces_no_repite_la_pregunta() {
        let mut session = Session::new();
        session.enter(0);

        let entries = vec![entrada(Source::System, PREGUNTA)];
        session.observe(&quieto(&entries));
        assert!(session.take_pending().is_some());

        session.observe(&quieto(&entries));
        assert_eq!(session.take_pending(), None, "la misma entrada se ha procesado dos veces");
    }

    /// Y el segundo mas facil: procesar de golpe lo que se dijo antes de entrar. La captura
    /// puede llevar abierta un rato solo para ver el medidor.
    #[test]
    fn lo_dicho_antes_de_entrar_no_cuenta() {
        let mut session = Session::new();
        let previas = vec![entrada(Source::System, "probando, uno, dos")];

        session.observe(&quieto(&previas));
        session.enter(previas.len());
        session.observe(&quieto(&previas));

        assert_eq!(session.take_pending(), None);
    }

    /// Dos turnos identicos seguidos son dos turnos. Si la sesion se acordase del texto en
    /// vez de la cuenta, el segundo desapareceria.
    #[test]
    fn dos_turnos_identicos_seguidos_son_dos() {
        let mut session = Session::new();
        session.enter(0);

        let mut entries = vec![entrada(Source::System, PREGUNTA)];
        session.observe(&quieto(&entries));
        session.take_pending();

        entries.push(entrada(Source::System, PREGUNTA));
        session.observe(&quieto(&entries));

        let pendiente = session.take_pending().expect("el segundo turno no se ha visto");
        assert!(pendiente.contains(PREGUNTA));
    }

    /// El evento de que el candidato empieza a hablar viene del VAD y no del texto, porque el
    /// texto llega segundos tarde. Sin esto, la pregunta se cerraria demasiado tarde y el
    /// turno siguiente del entrevistador se pegaria a la pregunta anterior.
    #[test]
    fn el_candidato_cierra_la_pregunta_en_cuanto_el_vad_lo_oye() {
        let mut session = Session::new();
        session.enter(0);

        let entries = vec![entrada(Source::System, PREGUNTA)];
        session.observe(&quieto(&entries));
        session.take_pending();
        session.suggestion(true);

        session.observe(&Snapshot { entries: &entries, mic_speaking: true, system_speaking: false });
        assert!(matches!(session.state(), State::Answering { .. }), "{:?}", session.state());
    }

    /// Solo el flanco: mientras el candidato siga hablando no se manda el evento otra vez.
    #[test]
    fn hablar_seguido_no_repite_el_evento() {
        let mut session = Session::new();
        session.enter(0);

        let entries = vec![entrada(Source::System, PREGUNTA)];
        session.observe(&quieto(&entries));
        session.take_pending();

        let hablando = Snapshot { entries: &entries, mic_speaking: true, system_speaking: false };
        session.observe(&hablando);
        session.observe(&hablando);

        // Sigue preparandose: que conteste de memoria no cancela la sugerencia.
        assert!(matches!(session.state(), State::Preparing { .. }), "{:?}", session.state());
    }

    #[test]
    fn salir_se_lleva_lo_que_quedara_pendiente() {
        let mut session = Session::new();
        session.enter(0);

        let entries = vec![entrada(Source::System, PREGUNTA)];
        session.observe(&quieto(&entries));

        session.leave();
        assert_eq!(session.take_pending(), None);
        assert_eq!(*session.state(), State::Off);
    }

    /// Una ampliacion sustituye a la pregunta a medias en vez de acumularse: preparar las dos
    /// seria gastar una pasada en la version incompleta.
    #[test]
    fn una_ampliacion_sustituye_a_la_pregunta_a_medias() {
        let mut session = Session::new();
        session.enter(0);

        let mut entries = vec![entrada(Source::System, PREGUNTA)];
        session.observe(&quieto(&entries));

        entries.push(entrada(Source::System, "Y ponme un ejemplo con cifras si las tienes"));
        session.observe(&quieto(&entries));

        let pendiente = session.take_pending().expect("hay pregunta");
        assert!(pendiente.starts_with(PREGUNTA), "{pendiente}");
        assert!(pendiente.ends_with("cifras si las tienes"), "{pendiente}");
        assert_eq!(session.take_pending(), None, "quedaba una segunda pregunta sin recoger");
    }
}
