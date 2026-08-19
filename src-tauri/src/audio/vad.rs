//! Deteccion de voz y de fin de turno con Silero (§7 y §10 del spec).
//!
//! El disparador de toda la entrevista es saber **cuando el entrevistador ha terminado de
//! preguntar**. Un umbral de decibelios no sirve para eso, y no es cuestion de afinarlo:
//! el ruido de una sala lo cruza y una pausa para pensar no, asi que el mismo numero dice
//! "sigue hablando" cuando hay una nevera zumbando y "ha terminado" cuando alguien coge
//! aire. Silero mira la forma de la senal, no su energia.
//!
//! El modulo esta partido en dos a proposito:
//!
//! - `Silero` habla con el modelo. Solo se puede probar con el fichero ONNX de verdad.
//! - `TurnDetector` decide, a partir de la probabilidad, cuando empieza y cuando acaba un
//!   turno. Es una maquina de estados sin dependencias, y se prueba con numeros escritos
//!   a mano.
//!
//! Esa division no es estetica: la parte que se puede equivocar en silencio —confundir una
//! pausa con el final de una pregunta— es la segunda, y es la que tiene tests.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use ort::session::Session;
use ort::value::Tensor;
use ringbuf::traits::{Consumer, Observer, Split};
use ringbuf::{HeapCons, HeapProd, HeapRb};

use crate::error::{AppError, AppResult};

/// Muestras por ventana. La v5 de Silero **exige** 512 a 16 kHz; no es una preferencia.
pub const FRAME_SAMPLES: usize = 512;

/// Lo que dura una ventana: 32 ms. Es la resolucion con la que se puede fechar el fin de
/// un turno, y por tanto el suelo de la latencia de esta etapa.
pub const FRAME_MS: usize = FRAME_SAMPLES * 1000 / crate::audio::resample::TARGET_HZ as usize;

/// De donde sale el modelo y cuanto pesa. Se descarga una vez, como los de embeddings.
pub const MODEL_URL: &str =
    "https://github.com/snakers4/silero-vad/raw/master/src/silero_vad/data/silero_vad.onnx";
pub const MODEL_FILE: &str = "silero_vad.onnx";

/// Huella del fichero que se probo. Un modelo que cambia por debajo cambia las
/// probabilidades y con ellas el fin de turno, y eso no puede pasar en silencio.
pub const MODEL_SHA256: &str = "1a153a22f4509e292a94e67d6f9b85e8deb25b4988682b7e174c65279d8788e3";

/// Tamano del estado recurrente de la v5: dos capas, un lote, 128 unidades.
const STATE_SHAPE: [i64; 3] = [2, 1, 128];

/// Muestras de la ventana anterior que hay que poner **por delante** de cada ventana.
///
/// Esto no sale de la documentacion, sale de una medicion. La v5 no recibe 512 muestras:
/// recibe 576, las 512 de la ventana mas 64 de contexto de la anterior. Alimentandola con
/// 512 el modelo no da error —la entrada es dinamica— y devuelve probabilidades, solo que
/// mucho mas bajas: la misma frase daba 0,54 sin nada por delante y 0,10 con un segundo de
/// silencio antes, que es como decir que no detectaba nada en cuanto habia una pausa. Con
/// el contexto puesto, esa misma frase da 0,98.
const CONTEXT_SAMPLES: usize = 64;

pub struct Silero {
    session: Session,
    /// El modelo es recurrente: la probabilidad de esta ventana depende de las anteriores.
    /// Perder el estado entre ventanas equivale a preguntarle por audio suelto.
    state: Vec<f32>,
    /// Las ultimas `CONTEXT_SAMPLES` muestras de la ventana anterior.
    context: Vec<f32>,
    /// Entrada reutilizada entre ventanas: contexto + ventana.
    input: Vec<f32>,
}

impl Silero {
    pub fn load(model: &Path) -> AppResult<Self> {
        let session = Session::builder()
            .and_then(|builder| builder.with_intra_threads(1))
            .and_then(|builder| builder.commit_from_file(model))
            .map_err(|err| AppError::Audio(format!("no se pudo cargar el VAD: {err}")))?;

        Ok(Self {
            session,
            state: vec![0.0; 2 * 128],
            context: vec![0.0; CONTEXT_SAMPLES],
            input: Vec::with_capacity(CONTEXT_SAMPLES + FRAME_SAMPLES),
        })
    }

    /// Probabilidad de que en esta ventana haya voz, de 0 a 1.
    pub fn probability(&mut self, frame: &[f32]) -> AppResult<f32> {
        if frame.len() != FRAME_SAMPLES {
            return Err(AppError::Audio(format!(
                "el VAD necesita ventanas de {FRAME_SAMPLES} muestras y llegaron {}",
                frame.len()
            )));
        }

        self.input.clear();
        self.input.extend_from_slice(&self.context);
        self.input.extend_from_slice(frame);

        let audio = Tensor::from_array((
            [1_i64, (CONTEXT_SAMPLES + FRAME_SAMPLES) as i64],
            self.input.clone(),
        ))
        .map_err(map_ort)?;
        let state = Tensor::from_array((STATE_SHAPE, self.state.clone())).map_err(map_ort)?;
        let rate = Tensor::from_array(([1_i64], vec![crate::audio::resample::TARGET_HZ as i64]))
            .map_err(map_ort)?;

        let outputs = self
            .session
            .run(ort::inputs!["input" => audio, "state" => state, "sr" => rate].map_err(map_ort)?)
            .map_err(map_ort)?;

        // El estado nuevo se guarda antes de mirar la probabilidad: si se olvida, el
        // modelo funciona igual de bien ventana a ventana y peor en todo lo demas, que es
        // la clase de fallo que no da error.
        let (_, next_state) = outputs["stateN"]
            .try_extract_raw_tensor::<f32>()
            .map_err(map_ort)?;
        self.state.clear();
        self.state.extend_from_slice(next_state);

        // El contexto de la siguiente ventana son las ultimas muestras de esta entrada.
        self.context.clear();
        self.context
            .extend_from_slice(&self.input[self.input.len() - CONTEXT_SAMPLES..]);

        let (_, probability) = outputs["output"]
            .try_extract_raw_tensor::<f32>()
            .map_err(map_ort)?;

        probability
            .first()
            .copied()
            .ok_or_else(|| AppError::Audio("el VAD no devolvió ninguna probabilidad".into()))
    }
}

fn map_ort(err: ort::Error) -> AppError {
    AppError::Audio(format!("error del VAD: {err}"))
}

/// A partir de cuanto se considera que una ventana tiene voz.
///
/// Es el valor de referencia de Silero, **heredado y no medido aqui**. Queda anotado como
/// tal: el numero que de verdad importa se calibra con audio de entrevistas reales, y
/// hasta entonces afinarlo a ojo seria repetir el error del umbral de similitud.
const SPEECH_THRESHOLD: f32 = 0.5;

/// Ventanas seguidas con voz para dar por empezado un turno. Dos son 64 ms: suficiente
/// para descartar un golpe de teclado y poco para no comerse la primera silaba.
const FRAMES_TO_START: usize = 2;

/// Silencio que cierra un turno. 700 ms es una pausa de fin de pregunta; 300 ms es coger
/// aire a mitad de frase y cerrar ahi cortaria al entrevistador mientras habla.
const SILENCE_MS_TO_END: usize = 700;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Turn {
    /// Nadie habla.
    Silent,
    /// Alguien esta hablando ahora mismo.
    Speaking,
}

/// Lo que ocurre al meter una ventana.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Nothing,
    /// Ha empezado a hablar.
    SpeechStarted,
    /// Ha terminado de hablar: es el disparador de la respuesta (§10). Lleva cuanto duro
    /// el turno, en milisegundos, sin contar el silencio final.
    TurnEnded { speech_ms: usize },
}

/// Decide cuando empieza y cuando acaba un turno a partir de las probabilidades.
///
/// La histeresis —dos condiciones distintas para entrar y para salir— es lo que evita que
/// una pregunta con pausas se parta en tres turnos.
#[derive(Debug)]
pub struct TurnDetector {
    turn: Turn,
    speech_frames: usize,
    silence_frames: usize,
    /// Ventanas con voz acumuladas en el turno en curso.
    turn_frames: usize,
}

impl Default for TurnDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl TurnDetector {
    pub fn new() -> Self {
        Self {
            turn: Turn::Silent,
            speech_frames: 0,
            silence_frames: 0,
            turn_frames: 0,
        }
    }

    pub fn turn(&self) -> Turn {
        self.turn
    }

    pub fn push(&mut self, probability: f32) -> Event {
        let has_voice = probability >= SPEECH_THRESHOLD;

        if has_voice {
            self.speech_frames += 1;
            self.silence_frames = 0;
        } else {
            self.silence_frames += 1;
            self.speech_frames = 0;
        }

        match self.turn {
            Turn::Silent => {
                if has_voice && self.speech_frames >= FRAMES_TO_START {
                    self.turn = Turn::Speaking;
                    // Las ventanas que abrieron el turno cuentan como habla: si no, un
                    // turno corto acabaria declarando cero milisegundos de voz.
                    self.turn_frames = self.speech_frames;
                    return Event::SpeechStarted;
                }
                Event::Nothing
            }
            Turn::Speaking => {
                if has_voice {
                    self.turn_frames += 1;
                    return Event::Nothing;
                }

                // Una pausa corta no cierra nada: sigue contando como parte del turno.
                if self.silence_frames * FRAME_MS >= SILENCE_MS_TO_END {
                    let speech_ms = self.turn_frames * FRAME_MS;
                    self.turn = Turn::Silent;
                    self.turn_frames = 0;
                    return Event::TurnEnded { speech_ms };
                }

                Event::Nothing
            }
        }
    }
}

/// El modelo y la maquina de estados, juntos.
///
/// Existe como pieza propia para que el hilo de audio en vivo y los tests midan **lo
/// mismo**. Aqui hubo un rato una politica de reiniciar el modelo tras un rato de
/// silencio, puesta para arreglar una perdida de sensibilidad que resulto ser otra cosa
/// —faltaba el contexto de 64 muestras—. Se quito al arreglar la causa: cada reinicio
/// mete un transitorio que abre turnos que nadie ha hablado.
pub struct VoiceTracker {
    silero: Silero,
    detector: TurnDetector,
    last_probability: f32,
}

impl VoiceTracker {
    pub fn new(model: &Path) -> AppResult<Self> {
        Ok(Self {
            silero: Silero::load(model)?,
            detector: TurnDetector::new(),
            last_probability: 0.0,
        })
    }

    pub fn push(&mut self, frame: &[f32]) -> AppResult<Event> {
        let probability = self.silero.probability(frame)?;
        self.last_probability = probability;
        Ok(self.detector.push(probability))
    }

    pub fn probability(&self) -> f32 {
        self.last_probability
    }

    pub fn turn(&self) -> Turn {
        self.detector.turn()
    }
}

/// Ruta del modelo dentro de la carpeta de modelos de la aplicacion.
pub fn model_path(models_dir: &Path) -> PathBuf {
    models_dir.join(MODEL_FILE)
}

/// Descarga el modelo si no esta, comprobando su huella.
///
/// Son 2,2 MB, frente al giga del modelo de embeddings, pero se descarga igual bajo
/// peticion y no al arrancar: §2 del spec dice sin rodeos que la app no depende de la red,
/// y una descarga automatica al abrir la ventana es exactamente eso.
///
/// Dos precauciones que no son ceremonia:
///
/// - **Se comprueba el SHA-256** contra el fichero que se probo. Un modelo distinto da
///   otras probabilidades, y con ellas otro fin de turno; enterarse de eso por un
///   comportamiento raro seria una tarde perdida.
/// - **Se escribe a un fichero temporal y se renombra al final.** Una descarga cortada a
///   la mitad dejaria un .onnx que existe, no carga, y parece un fallo del codigo.
pub async fn ensure_model(models_dir: &Path) -> AppResult<PathBuf> {
    let path = model_path(models_dir);
    if path.is_file() {
        return Ok(path);
    }

    std::fs::create_dir_all(models_dir)?;
    log::info!("descargando el modelo del VAD desde {MODEL_URL}");

    let response = reqwest::get(MODEL_URL)
        .await
        .map_err(|err| AppError::Audio(format!("no se pudo descargar el VAD: {err}")))?;
    if !response.status().is_success() {
        return Err(AppError::Audio(format!(
            "la descarga del VAD respondió {}",
            response.status()
        )));
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|err| AppError::Audio(format!("descarga del VAD interrumpida: {err}")))?;

    let digest = {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        hasher
            .finalize()
            .iter()
            .fold(String::new(), |mut acc, byte| {
                use std::fmt::Write;
                let _ = write!(acc, "{byte:02x}");
                acc
            })
    };

    if digest != MODEL_SHA256 {
        return Err(AppError::Audio(format!(
            "el modelo descargado no es el que se probó (huella {digest})"
        )));
    }

    let partial = path.with_extension("part");
    std::fs::write(&partial, &bytes)?;
    std::fs::rename(&partial, &path)?;

    log::info!("modelo del VAD listo en {}", path.display());
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cuantas ventanas hacen falta para acumular unos milisegundos de silencio.
    fn frames_for_ms(ms: usize) -> usize {
        ms.div_ceil(FRAME_MS)
    }

    fn feed(detector: &mut TurnDetector, probability: f32, frames: usize) -> Vec<Event> {
        (0..frames)
            .map(|_| detector.push(probability))
            .filter(|event| *event != Event::Nothing)
            .collect()
    }

    #[test]
    fn una_ventana_suelta_con_voz_no_abre_turno() {
        let mut detector = TurnDetector::new();
        assert_eq!(detector.push(0.9), Event::Nothing);
        assert_eq!(detector.turn(), Turn::Silent);
    }

    #[test]
    fn dos_ventanas_seguidas_abren_el_turno() {
        let mut detector = TurnDetector::new();
        detector.push(0.9);
        assert_eq!(detector.push(0.9), Event::SpeechStarted);
        assert_eq!(detector.turn(), Turn::Speaking);
    }

    /// El caso que justifica la histeresis: una pregunta con una pausa para pensar es
    /// **una** pregunta, no dos. Sin esto, la app respondería a media pregunta.
    #[test]
    fn una_pausa_corta_no_corta_la_pregunta() {
        let mut detector = TurnDetector::new();
        feed(&mut detector, 0.9, 10);

        let eventos = feed(&mut detector, 0.1, frames_for_ms(300));
        assert!(eventos.is_empty(), "una pausa de 300 ms cerró el turno");
        assert_eq!(detector.turn(), Turn::Speaking);

        // Y al volver la voz, sigue siendo el mismo turno.
        assert!(feed(&mut detector, 0.9, 5).is_empty());
    }

    #[test]
    fn el_silencio_largo_cierra_el_turno() {
        let mut detector = TurnDetector::new();
        feed(&mut detector, 0.9, 10);

        let eventos = feed(&mut detector, 0.1, frames_for_ms(SILENCE_MS_TO_END));
        assert_eq!(eventos.len(), 1);
        assert!(matches!(eventos[0], Event::TurnEnded { .. }));
        assert_eq!(detector.turn(), Turn::Silent);
    }

    /// La duracion que se informa es de **voz**, no de reloj: el silencio que cierra el
    /// turno no cuenta. Es lo que despues decide si un turno fue una pregunta o un "ajá".
    #[test]
    fn la_duracion_no_incluye_el_silencio_final() {
        let mut detector = TurnDetector::new();
        let ventanas = 10;
        feed(&mut detector, 0.9, ventanas);
        let eventos = feed(&mut detector, 0.1, frames_for_ms(SILENCE_MS_TO_END) + 20);

        match eventos.as_slice() {
            [Event::TurnEnded { speech_ms }] => {
                assert_eq!(*speech_ms, ventanas * FRAME_MS);
            }
            other => panic!("se esperaba un unico fin de turno y salio {other:?}"),
        }
    }

    /// Silencio interminable sin haber hablado nunca no puede producir un fin de turno.
    #[test]
    fn sin_haber_hablado_no_hay_fin_de_turno() {
        let mut detector = TurnDetector::new();
        assert!(feed(&mut detector, 0.0, 500).is_empty());
    }

    #[test]
    fn dos_turnos_seguidos_se_cuentan_por_separado() {
        let mut detector = TurnDetector::new();
        let silencio = frames_for_ms(SILENCE_MS_TO_END);

        feed(&mut detector, 0.9, 10);
        let primero = feed(&mut detector, 0.1, silencio);
        feed(&mut detector, 0.9, 4);
        let segundo = feed(&mut detector, 0.1, silencio);

        assert!(matches!(primero.as_slice(), [Event::TurnEnded { speech_ms }] if *speech_ms == 10 * FRAME_MS));
        assert!(matches!(segundo.as_slice(), [Event::TurnEnded { speech_ms }] if *speech_ms == 4 * FRAME_MS));
    }

    #[test]
    fn la_ventana_dura_treinta_y_dos_milisegundos() {
        assert_eq!(FRAME_MS, 32);
    }

    /// Voz de verdad, leida de un WAV, por el mismo camino que el audio en vivo.
    ///
    /// Y con el silencio por delante, que es lo que destapo el fallo que costo la tarde:
    /// **la v5 de Silero no recibe 512 muestras, recibe 576** —las 512 de la ventana mas
    /// 64 de contexto de la anterior—. Alimentandola con 512 no da error, porque la
    /// entrada es dinamica, y devuelve probabilidades plausibles pero bajas: la misma
    /// frase daba 0,54 sin nada por delante y 0,10 con un segundo de silencio antes. Un
    /// segundo de silencio es lo que hay entre dos preguntas de una entrevista.
    ///
    /// Por eso el test mide las dos, y no solo la facil.
    ///
    /// Genera el WAV con el sintetizador de Windows:
    /// `$v = New-Object System.Speech.Synthesis.SpeechSynthesizer; $v.SetOutputToWaveFile(...)`
    ///
    /// `INTERVIEW_COPILOT_VAD=<onnx> INTERVIEW_COPILOT_WAV=<wav> cargo test --lib -- --ignored --nocapture voz_de_un_wav`
    #[test]
    #[ignore = "necesita el modelo y un wav con voz"]
    fn voz_de_un_wav_con_y_sin_silencio_delante() {
        let model = std::env::var("INTERVIEW_COPILOT_VAD").expect("INTERVIEW_COPILOT_VAD");
        let wav = std::env::var("INTERVIEW_COPILOT_WAV").expect("INTERVIEW_COPILOT_WAV");
        let muestras = leer_wav_a_16k(&wav);

        for silencio_s in [0usize, 1, 5] {
            let mut audio =
                vec![0.0f32; crate::audio::resample::TARGET_HZ as usize * silencio_s];
            audio.extend_from_slice(&muestras);

            let mut tracker = VoiceTracker::new(Path::new(&model)).expect("cargar");
            let mut maxima = 0.0f32;
            let mut turnos = Vec::new();

            for ventana in audio.chunks_exact(FRAME_SAMPLES) {
                let evento = tracker.push(ventana).expect("inferir");
                maxima = maxima.max(tracker.probability());
                if let Event::TurnEnded { speech_ms } = evento {
                    turnos.push(speech_ms);
                }
            }

            println!("{silencio_s} s de silencio delante: maxima {maxima:.3}, turnos {turnos:?}");
            assert!(
                maxima > 0.9,
                "con {silencio_s} s de silencio delante el modelo apenas vio voz                  ({maxima:.3}): revisa el contexto de {CONTEXT_SAMPLES} muestras"
            );
        }
    }

    /// Lee un WAV PCM de 16 bits y lo deja en 16 kHz mono, por el mismo camino que el
    /// audio en vivo: si el remuestreo estropea la senal, este test se entera.
    fn leer_wav_a_16k(path: &str) -> Vec<f32> {
        let bytes = std::fs::read(path).expect("leer el wav");
        let canales = u16::from_le_bytes([bytes[22], bytes[23]]);
        let frecuencia = u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]);
        let data = bytes
            .windows(4)
            .position(|w| w == b"data")
            .map(|pos| pos + 8)
            .expect("el wav no trae bloque data");

        let crudas: Vec<f32> = bytes[data..]
            .chunks_exact(2)
            .map(|par| i16::from_le_bytes([par[0], par[1]]) as f32 / 32768.0)
            .collect();

        let mut muestras = Vec::new();
        crate::audio::resample::to_mono_16k(&crudas, canales, frecuencia, &mut muestras);
        println!(
            "{frecuencia} Hz, {canales} canales -> {} muestras a 16 kHz",
            muestras.len()
        );
        muestras
    }

    /// Con el modelo de verdad, sobre audio de verdad: silencio digital tiene que dar
    /// probabilidad baja, y un WAV con voz alta. Sin esto, todo lo de arriba es una
    /// maquina de estados alimentada por una suposicion.
    ///
    /// `INTERVIEW_COPILOT_VAD=<ruta al onnx> cargo test --lib -- --ignored --nocapture el_modelo`
    #[test]
    #[ignore = "necesita el modelo ONNX descargado"]
    fn el_modelo_distingue_silencio_de_voz() {
        let Ok(path) = std::env::var("INTERVIEW_COPILOT_VAD") else {
            panic!("define INTERVIEW_COPILOT_VAD con la ruta de silero_vad.onnx");
        };

        let mut silero = Silero::load(Path::new(&path)).expect("cargar el modelo");

        let silencio = vec![0.0f32; FRAME_SAMPLES];
        let mut ultima = 1.0;
        for _ in 0..10 {
            ultima = silero.probability(&silencio).expect("inferir");
        }
        println!("silencio: {ultima:.4}");
        assert!(ultima < SPEECH_THRESHOLD, "el silencio dio {ultima:.4}");

        // Un tono no es voz, pero es senal: sirve para comprobar que el modelo no dice
        // "voz" ante cualquier cosa que no sea silencio. Sesion nueva para no arrastrar
        // el estado del silencio anterior: el modelo es recurrente.
        let mut silero = Silero::load(Path::new(&path)).expect("cargar el modelo");
        let tono: Vec<f32> = (0..FRAME_SAMPLES)
            .map(|index| (std::f32::consts::TAU * 440.0 * index as f32 / 16_000.0).sin() * 0.5)
            .collect();
        let mut con_tono = 0.0;
        for _ in 0..10 {
            con_tono = silero.probability(&tono).expect("inferir");
        }
        println!("tono de 440 Hz: {con_tono:.4}");
    }
}

// ---------------------------------------------------------------------------
// El VAD en vivo, colgado de una captura
// ---------------------------------------------------------------------------

/// Segundos de audio que caben en la cola entre la tarjeta y el VAD.
///
/// Dos segundos es holgado: el VAD consume ventanas de 32 ms y solo se retrasaria si el
/// equipo se atasca. Si aun asi se llena, se tiran las muestras **nuevas** y se cuentan;
/// la alternativa —bloquear— seria cortar el audio para no perder un dato del medidor.
const QUEUE_SECONDS: usize = 2;

/// Lo que el hilo del VAD escribe y la UI lee.
#[derive(Debug, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VadState {
    pub turn: Turn,
    /// Probabilidad de la ultima ventana. Se ensena para poder ver el margen con el que
    /// se esta decidiendo, en vez de un si o un no sin contexto.
    pub probability: f32,
    /// La mas alta desde que arranco la captura.
    ///
    /// No es adorno: es el dato con el que se sabra si el umbral esta bien puesto. Medido
    /// el 2026-08-19, la voz sintetica de Windows se queda en 0,55 —justo encima del 0,5
    /// de referencia— y una voz humana deberia irse mucho mas arriba. Sin este numero, un
    /// VAD que casi no dispara y uno que dispara de sobra se ven igual en pantalla.
    pub max_probability: f32,
    /// Duracion del ultimo turno cerrado, en milisegundos de voz.
    pub last_turn_ms: Option<usize>,
    pub turns: usize,
    /// Ventanas que se perdieron porque la cola se lleno. Distinto de cero significa que
    /// el VAD no vio todo el audio, y eso hay que saberlo antes de fiarse de un turno.
    pub dropped: usize,
    /// Diagnostico: la muestra mas alta que ha visto el VAD, para distinguir "no hay voz"
    /// de "no esta llegando el audio".
    pub peak_in: f32,
}

impl VadState {
    fn idle() -> Self {
        Self {
            turn: Turn::Silent,
            probability: 0.0,
            max_probability: 0.0,
            last_turn_ms: None,
            turns: 0,
            dropped: 0,
            peak_in: 0.0,
        }
    }
}

/// Extremo de escritura de la cola, para el hilo de audio.
pub type SampleSink = HeapProd<f32>;

/// El VAD corriendo sobre una captura: una cola, un hilo y un estado compartido.
pub struct LiveVad {
    shared: Arc<Mutex<VadState>>,
    /// Muestras perdidas por cola llena. Es un atomico y no un campo del estado porque
    /// quien lo incrementa es la llamada de retorno de audio, y ahi no se puede tomar un
    /// mutex sin arriesgarse a cortar el sonido.
    dropped: Arc<AtomicUsize>,
    stopping: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl LiveVad {
    /// Crea la cola y arranca el hilo. Devuelve el extremo de escritura, que tiene que
    /// acabar dentro de la llamada de retorno de audio.
    pub fn start(model: &Path) -> AppResult<(Self, SampleSink)> {
        // Se carga aqui y no en el hilo para que un modelo que falta o esta corrupto sea
        // un error de "arrancar la captura" y no un hilo que muere en silencio.
        let mut tracker = VoiceTracker::new(model)?;

        let (producer, mut consumer): (HeapProd<f32>, HeapCons<f32>) =
            HeapRb::<f32>::new(crate::audio::resample::TARGET_HZ as usize * QUEUE_SECONDS).split();

        let shared = Arc::new(Mutex::new(VadState::idle()));
        let stopping = Arc::new(AtomicBool::new(false));

        let thread_shared = Arc::clone(&shared);
        let thread_stopping = Arc::clone(&stopping);

        let thread = std::thread::Builder::new()
            .name("vad".into())
            .spawn(move || {
                let mut frame = [0.0f32; FRAME_SAMPLES];

                while !thread_stopping.load(Ordering::Relaxed) {
                    if consumer.occupied_len() < FRAME_SAMPLES {
                        // Media ventana: dormir menos que eso desperdicia CPU y dormir
                        // mas anade latencia al fin de turno.
                        std::thread::sleep(std::time::Duration::from_millis(
                            (FRAME_MS / 2) as u64,
                        ));
                        continue;
                    }

                    consumer.pop_slice(&mut frame);
                    let event = match tracker.push(&frame) {
                        Ok(event) => event,
                        Err(err) => {
                            log::warn!("el VAD dejo de responder: {err}");
                            break;
                        }
                    };

                    let peak = frame.iter().fold(0.0f32, |max, s| max.max(s.abs()));
                    if let Ok(mut state) = thread_shared.lock() {
                        state.probability = tracker.probability();
                        state.max_probability = state.max_probability.max(tracker.probability());
                        state.peak_in = state.peak_in.max(peak);
                        state.turn = tracker.turn();
                        if let Event::TurnEnded { speech_ms } = event {
                            state.last_turn_ms = Some(speech_ms);
                            state.turns += 1;
                        }
                    }
                }
            })
            .map_err(|err| AppError::Audio(format!("no se pudo crear el hilo del VAD: {err}")))?;

        Ok((
            Self {
                shared,
                dropped: Arc::new(AtomicUsize::new(0)),
                stopping,
                thread: Some(thread),
            },
            producer,
        ))
    }

    pub fn state(&self) -> VadState {
        let mut state = self
            .shared
            .lock()
            .map(|state| *state)
            .unwrap_or_else(|_| VadState::idle());
        state.dropped = self.dropped.load(Ordering::Relaxed);
        state
    }

    /// El contador de muestras perdidas, para que lo incremente el hilo de audio.
    pub fn dropped_counter(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.dropped)
    }
}

impl Drop for LiveVad {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            if thread.join().is_err() {
                log::warn!("el hilo del VAD termino en panic");
            }
        }
    }
}
