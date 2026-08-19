//! El hilo que transcribe turnos mientras la entrevista sigue.
//!
//! El VAD cierra un turno y manda su audio aqui; whisper lo transcribe cuando puede y el
//! texto aparece en la lista. Van por un canal y no por una llamada directa porque
//! transcribir tarda segundos y el hilo del VAD tiene que seguir mirando el audio que
//! entra: si se bloqueara, la siguiente pregunta se perderia entera.
//!
//! El modelo se carga **en el primer turno**, no al arrancar la captura. Son ~200 MB y
//! varios segundos, y en esta maquina eso es la diferencia entre poder abrir la pantalla
//! de audio para elegir microfono y tener que esperar a que cargue un modelo que quiza no
//! haga falta.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use crate::audio::Source;
use crate::error::{AppError, AppResult};
use crate::stt::{LocalWhisper, SttProvider};

/// Cuantos turnos se conservan en pantalla. Lo suficiente para seguir una conversacion sin
/// que la memoria crezca durante una entrevista de una hora.
const MAX_ENTRIES: usize = 50;

/// Un turno ya transcrito.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub source: Source,
    pub text: String,
    /// Cuanto audio traia el turno.
    pub audio_ms: usize,
    /// Cuanto tardo whisper. Es el numero que decide si el modo LOCAL es usable (§10), y
    /// por eso se ensena en vez de guardarse en un log.
    pub took_ms: u128,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptState {
    pub entries: Vec<Entry>,
    /// Turnos esperando a que whisper termine con el anterior. Si esto crece, el equipo no
    /// da abasto y hay que bajar de modelo.
    pub pending: usize,
    pub model: String,
    /// El modelo esta cargado en memoria.
    pub loaded: bool,
    pub error: Option<String>,
}

#[derive(Debug)]
struct Shared {
    entries: VecDeque<Entry>,
    pending: usize,
    loaded: bool,
    error: Option<String>,
}

struct Job {
    source: Source,
    samples: Vec<f32>,
}

pub struct Transcriber {
    model_id: String,
    sender: Option<mpsc::Sender<Job>>,
    shared: Arc<Mutex<Shared>>,
    thread: Option<JoinHandle<()>>,
}

impl Transcriber {
    /// Arranca el hilo. No carga el modelo todavia: eso ocurre con el primer turno.
    pub fn start(model_path: PathBuf, model_id: &str) -> AppResult<Self> {
        let (sender, receiver) = mpsc::channel::<Job>();
        let shared = Arc::new(Mutex::new(Shared {
            entries: VecDeque::new(),
            pending: 0,
            loaded: false,
            error: None,
        }));

        let thread_shared = Arc::clone(&shared);
        let thread_id = model_id.to_owned();

        let thread = std::thread::Builder::new()
            .name("transcripcion".into())
            .spawn(move || {
                let mut whisper: Option<LocalWhisper> = None;

                // El canal se cierra al soltar el `Transcriber`, y entonces esto termina.
                while let Ok(job) = receiver.recv() {
                    if whisper.is_none() {
                        match LocalWhisper::load(&model_path, &thread_id) {
                            Ok(cargado) => {
                                log::info!("transcribiendo con {}", cargado.id());
                                whisper = Some(cargado);
                                if let Ok(mut shared) = thread_shared.lock() {
                                    shared.loaded = true;
                                }
                            }
                            Err(err) => {
                                if let Ok(mut shared) = thread_shared.lock() {
                                    shared.error = Some(err.to_string());
                                    shared.pending = shared.pending.saturating_sub(1);
                                }
                                continue;
                            }
                        }
                    }

                    let Some(modelo) = whisper.as_mut() else {
                        continue;
                    };

                    let audio_ms = job.samples.len() * 1000 / 16_000;
                    let empezo = std::time::Instant::now();
                    // El idioma se fija en espanol hasta que exista el selector de §14.
                    // Dejar que lo detecte cuesta una pasada mas y acierta peor con frases
                    // cortas, que es justo lo que son los turnos de una entrevista.
                    let resultado = modelo.transcribe(&job.samples, Some("es"));
                    let took_ms = empezo.elapsed().as_millis();

                    if let Ok(mut shared) = thread_shared.lock() {
                        shared.pending = shared.pending.saturating_sub(1);
                        match resultado {
                            Ok(text) if !text.is_empty() => {
                                shared.entries.push_back(Entry {
                                    source: job.source,
                                    text,
                                    audio_ms,
                                    took_ms,
                                });
                                while shared.entries.len() > MAX_ENTRIES {
                                    shared.entries.pop_front();
                                }
                            }
                            // Un turno del que no sale texto no es un error: puede ser una
                            // tos. No se apunta como fallo ni se ensena una linea vacia.
                            Ok(_) => log::info!("turno de {audio_ms} ms sin texto"),
                            Err(err) => {
                                log::warn!("whisper falló: {err}");
                                shared.error = Some(err.to_string());
                            }
                        }
                    }
                }
            })
            .map_err(|err| {
                AppError::Invalid(format!("no se pudo crear el hilo de transcripción: {err}"))
            })?;

        Ok(Self {
            model_id: model_id.to_owned(),
            sender: Some(sender),
            shared,
            thread: Some(thread),
        })
    }

    pub fn state(&self) -> TranscriptState {
        let shared = self.shared.lock();
        match shared {
            Ok(shared) => TranscriptState {
                entries: shared.entries.iter().cloned().collect(),
                pending: shared.pending,
                model: self.model_id.clone(),
                loaded: shared.loaded,
                error: shared.error.clone(),
            },
            Err(_) => TranscriptState {
                entries: Vec::new(),
                pending: 0,
                model: self.model_id.clone(),
                loaded: false,
                error: Some("el hilo de transcripción se rompió".into()),
            },
        }
    }

    /// Un emisor suelto para que el hilo del VAD mande turnos sin conocer nada de esto.
    pub fn sender(&self) -> Option<TurnSink> {
        self.sender.as_ref().map(|sender| TurnSink {
            sender: sender.clone(),
            shared: Arc::clone(&self.shared),
        })
    }
}

impl Drop for Transcriber {
    fn drop(&mut self) {
        // Soltar el emisor cierra el canal y con el, el bucle del hilo.
        self.sender.take();
        if let Some(thread) = self.thread.take() {
            if thread.join().is_err() {
                log::warn!("el hilo de transcripción terminó en panic");
            }
        }
    }
}

/// Por donde el VAD manda los turnos cerrados.
#[derive(Clone)]
pub struct TurnSink {
    sender: mpsc::Sender<Job>,
    shared: Arc<Mutex<Shared>>,
}

impl TurnSink {
    pub fn submit(&self, source: Source, samples: Vec<f32>) -> bool {
        if let Ok(mut shared) = self.shared.lock() {
            shared.pending += 1;
        }
        self.sender.send(Job { source, samples }).is_ok()
    }
}
