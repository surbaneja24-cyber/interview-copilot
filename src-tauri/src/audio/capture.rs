//! Captura de audio con cpal (WASAPI en Windows): micrófono y audio del sistema.
//!
//! **El audio del sistema no lleva codigo aparte.** WASAPI graba lo que suena abriendo un
//! dispositivo de *salida* en modo captura, y cpal lo hace solo: si construyes un flujo de
//! entrada sobre un dispositivo `eRender`, anade `AUDCLNT_STREAMFLAGS_LOOPBACK` por su
//! cuenta. Lo unico que cambia entre las dos fuentes es de que lista sale el dispositivo y
//! de donde sale su configuracion; el resto —el hilo, el medidor, el cierre— es el mismo.
//!
//! Separar por fuente es ademas como se distingue quien habla en el MVP 1: lo que entra
//! por el microfono es el usuario y lo que entra por el loopback es el entrevistador. No
//! hace falta reconocer voces, que es un problema mucho mas caro. La limitacion, y la UI
//! la dice: con altavoces en vez de auriculares, la voz del usuario vuelve por el loopback
//! y esa separacion deja de separar.
//!
//! Dos cosas condicionan la forma de este modulo:
//!
//! 1. **`cpal::Stream` no es `Send`.** En Windows la sesion de audio esta atada al hilo
//!    que la abrio, asi que el stream no puede guardarse en el estado de Tauri, que se
//!    comparte entre hilos. Vive en un hilo propio que lo crea y se queda bloqueado hasta
//!    que le mandan parar.
//! 2. **La llamada de retorno de audio no puede bloquearse.** Se ejecuta con un plazo de
//!    milisegundos; un mutex, una asignacion de memoria o un `log::info!` ahi producen
//!    cortes. Por eso lo unico que hace es medir y escribir en atomicos (ver `level`).
//!
//! El nivel no se emite por un `Channel` de Tauri como hace el LLM: la llamada de retorno
//! entra cada 10 ms y serian cien mensajes por segundo para una barra que se repinta
//! sesenta veces. La UI pregunta cuando va a dibujar.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use std::path::PathBuf;
use std::str::FromStr;
use std::sync::atomic::{AtomicUsize, Ordering};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{DeviceId, FromSample, Sample, SizedSample};
use ringbuf::traits::Producer;

use crate::audio::level::{Level, Meter};
use crate::audio::resample;
use crate::audio::vad::{LiveVad, SampleSink, VadState};
use crate::stt::transcriber::TurnSink;
use crate::error::{AppError, AppResult};

/// De donde se captura.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Source {
    /// El microfono: la voz del usuario.
    Mic,
    /// Lo que el equipo reproduce: la voz del entrevistador en la videollamada.
    System,
}

impl Source {
    fn label(self) -> &'static str {
        match self {
            Self::Mic => "micrófono",
            Self::System => "audio del sistema",
        }
    }
}

/// Un dispositivo de entrada tal y como lo ve el sistema.
///
/// Se guarda el identificador y no el nombre: cpal 0.17 da un `DeviceId` estable entre
/// reinicios y reconexiones, y el nombre no distingue dos tarjetas iguales. Es lo que el
/// dia de manana permite recordar en los ajustes que microfono eligio el usuario.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InputDevice {
    pub id: String,
    pub name: String,
    pub is_default: bool,
    /// Canales y frecuencia con los que abriria, para poder ensenar por que un
    /// dispositivo no vale antes de intentarlo.
    pub channels: u16,
    pub sample_rate: u32,
}

/// Lo que la UI necesita saber de la captura, incluido el nivel.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureStatus {
    pub source: Source,
    pub capturing: bool,
    pub device: Option<String>,
    pub sample_rate: u32,
    pub channels: u16,
    pub level: Level,
    /// Muestras recibidas. Un dispositivo que abre pero no entrega nada —un micrófono
    /// silenciado por hardware, un permiso denegado— se ve aqui y solo aqui.
    pub frames: u64,
    /// Fallo posterior al arranque: el cable se fue, el dispositivo desaparecio. cpal lo
    /// avisa por una llamada aparte que nadie esta escuchando, asi que se guarda.
    pub error: Option<String>,
    /// Estado del detector de voz, si hay modelo descargado. `None` significa que no se
    /// esta detectando nada, que no es lo mismo que "no hay voz".
    pub vad: Option<VadState>,
    /// Lo que tardo el dispositivo en decir que estaba abierto, desde que se pidio.
    pub opened_ms: Option<u64>,
    /// Lo que tardo en llegar la primera muestra.
    ///
    /// Es la ventana muerta del arranque: lo que se hable antes de esto no se descarta,
    /// no existe. Va al estado y no solo al log porque es la diferencia entre "el
    /// microfono no me oye" y "el microfono tarda medio segundo en encenderse", que en
    /// pantalla se parecen y llevan a sitios opuestos.
    pub first_sample_ms: Option<u64>,
}

impl CaptureStatus {
    pub fn idle(source: Source) -> Self {
        Self {
            source,
            capturing: false,
            device: None,
            sample_rate: 0,
            channels: 0,
            level: Level::SILENT,
            frames: 0,
            error: None,
            vad: None,
            opened_ms: None,
            first_sample_ms: None,
        }
    }
}

/// Dispositivos que se pueden abrir para una fuente.
///
/// Para el audio del sistema son las **salidas**: los altavoces o los auriculares por los
/// que sale la videollamada. Suena al reves y es exactamente como funciona el loopback.
pub fn devices(source: Source) -> AppResult<Vec<InputDevice>> {
    let host = cpal::default_host();

    let (default_id, listed) = match source {
        Source::Mic => (
            host.default_input_device().and_then(|d| d.id().ok()),
            host.input_devices().map(|devices| devices.collect::<Vec<_>>()),
        ),
        Source::System => (
            host.default_output_device().and_then(|d| d.id().ok()),
            host.output_devices().map(|devices| devices.collect::<Vec<_>>()),
        ),
    };

    let listed = listed.map_err(|err| {
        AppError::Audio(format!(
            "no se pudieron listar los dispositivos de {}: {err}",
            source.label()
        ))
    })?;

    let mut out = Vec::new();
    for device in listed {
        // Un dispositivo que no sabe decir ni quien es ni con que configuracion abre no
        // se puede ofrecer, pero tampoco puede tumbar la lista de los demas.
        let (Ok(id), Ok(description), Ok(config)) =
            (device.id(), device.description(), default_config(&device, source))
        else {
            continue;
        };

        out.push(InputDevice {
            is_default: Some(&id) == default_id.as_ref(),
            id: id.to_string(),
            name: description.name().to_owned(),
            channels: config.channels(),
            sample_rate: config.sample_rate(),
        });
    }

    Ok(out)
}

/// La configuracion con la que abre cada fuente. Para el loopback es la de *salida*: el
/// flujo lo marca el formato con el que el sistema esta reproduciendo.
fn default_config(
    device: &cpal::Device,
    source: Source,
) -> Result<cpal::SupportedStreamConfig, cpal::DefaultStreamConfigError> {
    match source {
        Source::Mic => device.default_input_config(),
        Source::System => device.default_output_config(),
    }
}

pub struct Recorder {
    source: Source,
    /// El detector de voz colgado de esta captura, si habia modelo. Al soltarlo se para
    /// su hilo, asi que vive exactamente lo que vive la captura.
    vad: Option<LiveVad>,
    device: String,
    sample_rate: u32,
    channels: u16,
    meter: Arc<Meter>,
    failure: Arc<Mutex<Option<String>>>,
    /// El hilo duenno del stream. Vive mientras viva esta estructura.
    thread: Option<JoinHandle<()>>,
    /// Al soltarlo, el hilo de captura despierta y termina.
    stop: Option<mpsc::Sender<()>>,
}

/// Lo que el hilo devuelve cuando ha conseguido abrir el dispositivo.
struct Opened {
    device: String,
    sample_rate: u32,
    channels: u16,
}

impl Recorder {
    /// Abre el dispositivo pedido, o el que el sistema tenga por defecto.
    ///
    /// Devuelve error si el dispositivo no existe o no se puede abrir, y lo hace **antes**
    /// de dar la captura por buena: un boton que se pone verde y no captura nada es peor
    /// que un error.
    /// `vad_model` es opcional: sin el, la captura funciona igual y no hay deteccion de
    /// voz. Es deliberado — el medidor y la eleccion de dispositivo tienen que poder
    /// usarse antes de descargar nada.
    pub fn start(
        source: Source,
        requested: Option<String>,
        vad_model: Option<PathBuf>,
        turns: Option<TurnSink>,
    ) -> AppResult<Self> {
        let meter = Arc::new(Meter::new());
        let failure: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

        // Se arranca antes que el audio para que un modelo corrupto sea un error al
        // pulsar "Escuchar" y no un hilo que muere sin que nadie se entere.
        let (vad, sink) = match vad_model {
            Some(model) => {
                let (live, queue) = LiveVad::start(&model, source, turns)?;
                let dropped = live.dropped_counter();
                (Some(live), Some((queue, dropped)))
            }
            None => (None, None),
        };

        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let (ready_tx, ready_rx) = mpsc::channel::<AppResult<Opened>>();

        let thread_meter = Arc::clone(&meter);
        let thread_failure = Arc::clone(&failure);

        let thread = std::thread::Builder::new()
            .name("captura-audio".into())
            .spawn(move || {
                match open_stream(source, requested.as_deref(), &thread_meter, &thread_failure, sink) {
                    Err(err) => {
                        let _ = ready_tx.send(Err(err));
                    }
                    Ok((stream, keep_alive, opened)) => {
                        if let Err(err) = stream.play() {
                            let _ = ready_tx.send(Err(AppError::Audio(format!(
                                "el dispositivo abrió pero no arrancó: {err}"
                            ))));
                            return;
                        }
                        thread_meter.mark_opened();
                        if let Some(silence) = keep_alive.as_ref() {
                            if let Err(err) = silence.play() {
                                // Sin el flujo mudo el loopback sigue funcionando; lo que
                                // se pierde es el silencio, y eso se ve en el medidor.
                                log::warn!("no arrancó el flujo mudo del loopback: {err}");
                            }
                        }

                        let _ = ready_tx.send(Ok(opened));
                        // Bloquea hasta que se suelte el emisor: es el hilo duenno de los
                        // flujos y tiene que seguir vivo mientras haya captura.
                        let _ = stop_rx.recv();
                        drop(keep_alive);
                        drop(stream);
                    }
                }
            })
            .map_err(|err| AppError::Audio(format!("no se pudo crear el hilo de audio: {err}")))?;

        let opened = ready_rx
            .recv()
            .map_err(|_| AppError::Audio("el hilo de audio murió al arrancar".into()))??;

        log::info!(
            "capturando {} de \"{}\" a {} Hz, {} canales",
            source.label(),
            opened.device,
            opened.sample_rate,
            opened.channels
        );

        Ok(Self {
            source,
            vad,
            device: opened.device,
            sample_rate: opened.sample_rate,
            channels: opened.channels,
            meter,
            failure,
            thread: Some(thread),
            stop: Some(stop_tx),
        })
    }

    pub fn status(&self) -> CaptureStatus {
        CaptureStatus {
            source: self.source,
            // Mientras exista este `Recorder` hay un hilo con el dispositivo abierto:
            // pararlo es soltarlo, no apagar un interruptor.
            capturing: true,
            device: Some(self.device.clone()),
            sample_rate: self.sample_rate,
            channels: self.channels,
            level: self.meter.read(),
            frames: self.meter.frames(),
            error: self
                .failure
                .lock()
                .ok()
                .and_then(|failure| failure.clone()),
            vad: self.vad.as_ref().map(LiveVad::state),
            opened_ms: self.meter.opened_ms(),
            first_sample_ms: self.meter.first_sample_ms(),
        }
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        // El emisor se suelta con la estructura y eso despierta al hilo; esperarlo aqui
        // garantiza que el dispositivo queda libre antes de intentar abrirlo otra vez.
        self.stop.take();
        if let Some(thread) = self.thread.take() {
            if thread.join().is_err() {
                log::warn!("el hilo de captura termino en panic");
            }
        }
    }
}

type Sink = (SampleSink, Arc<AtomicUsize>);

fn open_stream(
    source: Source,
    requested: Option<&str>,
    meter: &Arc<Meter>,
    failure: &Arc<Mutex<Option<String>>>,
    sink: Option<Sink>,
) -> AppResult<(cpal::Stream, Option<cpal::Stream>, Opened)> {
    let host = cpal::default_host();

    let device = match requested {
        Some(id) => {
            let id = DeviceId::from_str(id).map_err(|err| {
                AppError::Audio(format!("identificador de dispositivo ilegible: {err}"))
            })?;
            host.device_by_id(&id).ok_or_else(|| {
                AppError::Audio(format!("ese dispositivo de {} ya no está conectado", source.label()))
            })?
        }
        None => match source {
            Source::Mic => host
                .default_input_device()
                .ok_or_else(|| AppError::Audio("este equipo no tiene micrófono".into()))?,
            Source::System => host.default_output_device().ok_or_else(|| {
                AppError::Audio("este equipo no tiene salida de audio que escuchar".into())
            })?,
        },
    };

    let name = device
        .description()
        .map(|description| description.name().to_owned())
        .map_err(|err| AppError::Audio(format!("el dispositivo no dice su nombre: {err}")))?;
    let config = default_config(&device, source).map_err(|err| {
        AppError::Audio(format!("no se pudo leer la configuración de \"{name}\": {err}"))
    })?;

    let opened = Opened {
        device: name.clone(),
        sample_rate: config.sample_rate(),
        channels: config.channels(),
    };

    // Aqui no hay rama para el loopback a proposito: sobre un dispositivo de salida, cpal
    // pone `AUDCLNT_STREAMFLAGS_LOOPBACK` el solo. Escribir el flag a mano seria repetir
    // lo que ya hace la biblioteca y quedarnos con la version que envejece peor.
    let stream_config: cpal::StreamConfig = config.clone().into();
    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => build::<f32>(&device, &stream_config, meter, failure, sink),
        cpal::SampleFormat::I16 => build::<i16>(&device, &stream_config, meter, failure, sink),
        cpal::SampleFormat::U16 => build::<u16>(&device, &stream_config, meter, failure, sink),
        // Se enumeran los tres que WASAPI entrega en la practica. Un formato nuevo tiene
        // que dar un error visible, no una captura muda.
        other => {
            return Err(AppError::Audio(format!(
                "formato de muestra no soportado: {other}"
            )))
        }
    }?;

    // Medido el 2026-08-19: en silencio, el loopback no entrega **ni una muestra**. WASAPI
    // solo produce datos mientras el dispositivo de salida esta activo, asi que con la
    // videollamada callada el medidor se congelaria y el reloj de la transcripcion tendria
    // agujeros. El apano estandar es mantener abierto un flujo de reproduccion mudo sobre
    // el mismo dispositivo: no se oye nada —son ceros— y basta para que el reloj no pare.
    let keep_alive = match source {
        Source::Mic => None,
        Source::System => match silence(&device, &config) {
            Ok(stream) => Some(stream),
            Err(err) => {
                // No es motivo para no capturar: sin esto se pierde el silencio, no la voz.
                log::warn!("no se pudo abrir el flujo mudo del loopback: {err}");
                None
            }
        },
    };

    Ok((stream, keep_alive, opened))
}

/// Flujo de reproduccion que solo escribe ceros. Ver la nota de `open_stream`.
fn silence(
    device: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
) -> AppResult<cpal::Stream> {
    fn build_silence<T: SizedSample>(
        device: &cpal::Device,
        config: &cpal::StreamConfig,
    ) -> AppResult<cpal::Stream> {
        device
            .build_output_stream(
                config,
                move |data: &mut [T], _| data.fill(T::EQUILIBRIUM),
                |err| log::warn!("error del flujo mudo: {err}"),
                None,
            )
            .map_err(|err| AppError::Audio(format!("no se pudo abrir el flujo mudo: {err}")))
    }

    let format = config.sample_format();
    let config: cpal::StreamConfig = config.clone().into();

    match format {
        cpal::SampleFormat::F32 => build_silence::<f32>(device, &config),
        cpal::SampleFormat::I16 => build_silence::<i16>(device, &config),
        cpal::SampleFormat::U16 => build_silence::<u16>(device, &config),
        other => Err(AppError::Audio(format!(
            "formato de muestra no soportado: {other}"
        ))),
    }
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    meter: &Arc<Meter>,
    failure: &Arc<Mutex<Option<String>>>,
    sink: Option<Sink>,
) -> AppResult<cpal::Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let meter = Arc::clone(meter);
    let on_error = Arc::clone(failure);
    let mut buffer: Vec<f32> = Vec::new();
    let mut mono: Vec<f32> = Vec::new();
    let channels = config.channels;
    let sample_rate = config.sample_rate;
    let mut sink = sink;

    device
        .build_input_stream(
            config,
            move |data: &[T], _| {
                // El buffer vive en la clausura y se reutiliza: reservar memoria dentro
                // de la llamada de retorno de audio es lo que produce cortes.
                buffer.clear();
                buffer.extend(data.iter().map(|sample| f32::from_sample(*sample)));
                meter.push(&buffer);

                // El VAD trabaja a 16 kHz mono: se convierte aqui, una vez, en vez de
                // mandarle el doble de datos y que los tire el.
                if let Some((queue, dropped)) = sink.as_mut() {
                    mono.clear();
                    resample::to_mono_16k(&buffer, channels, sample_rate, &mut mono);
                    let pushed = queue.push_slice(&mono);
                    if pushed < mono.len() {
                        // La cola llena significa que el VAD no da abasto. Se tira lo
                        // nuevo y se cuenta: un detector que no vio parte del audio no
                        // puede presentarse como si lo hubiera visto todo.
                        dropped.fetch_add(mono.len() - pushed, Ordering::Relaxed);
                    }
                }
            },
            move |err| {
                // Aqui no se puede hacer nada mas que dejar constancia: la UI lo lee al
                // preguntar por el estado. Un fallo de audio que solo va al log es un
                // fallo que el usuario descubre en mitad de la entrevista.
                log::warn!("error del flujo de audio: {err}");
                if let Ok(mut slot) = on_error.lock() {
                    *slot = Some(err.to_string());
                }
            },
            None,
        )
        .map_err(|err| AppError::Audio(format!("no se pudo abrir el flujo de audio: {err}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Por encima de esto ya no hay duda de que el modelo esta viendo voz. No es el umbral
    /// de decision del VAD —ese es cosa de `vad`— sino lo que se le exige a este test.
    const SPEECH_CERTAIN: f32 = 0.9;

    /// No comprueba que haya microfono —una maquina sin tarjeta de sonido es legitima—
    /// sino que enumerar no revienta y que lo que sale tiene sentido.
    #[test]
    fn enumerar_no_revienta() {
        for source in [Source::Mic, Source::System] {
            let listed = devices(source).expect("enumerar dispositivos");
            println!("{} dispositivos para {}", listed.len(), source.label());

            for device in &listed {
                println!(
                    "  {} [{}] — {} Hz, {} canales{}",
                    device.name,
                    device.id,
                    device.sample_rate,
                    device.channels,
                    if device.is_default { " (por defecto)" } else { "" }
                );
                assert!(!device.name.is_empty());
                assert!(!device.id.is_empty(), "sin identificador no se puede volver a abrir");
                assert!(device.sample_rate > 0, "una frecuencia de cero no es abrible");
                assert!(device.channels > 0);
            }

            assert!(
                listed.iter().filter(|device| device.is_default).count() <= 1,
                "no puede haber dos dispositivos por defecto"
            );
        }
    }

    /// Las dos fuentes salen de listas distintas: el microfono de las entradas y el audio
    /// del sistema de las salidas. Confundirlas es el fallo tipico de esta parte.
    #[test]
    fn el_audio_del_sistema_sale_de_las_salidas() {
        let micros = devices(Source::Mic).expect("entradas");
        let salidas = devices(Source::System).expect("salidas");

        let en_micros: Vec<&str> = micros.iter().map(|d| d.id.as_str()).collect();
        for salida in &salidas {
            assert!(
                !en_micros.contains(&salida.id.as_str()),
                "\"{}\" aparece en las dos listas",
                salida.name
            );
        }
    }

    /// Abre y cierra de verdad. Va marcado `#[ignore]` porque toma el microfono del equipo
    /// y en una maquina sin entrada de audio fallaria por el motivo equivocado.
    ///
    /// `cargo test --lib -- --ignored --nocapture captura_medio_segundo`
    #[test]
    #[ignore = "toma el micrófono del equipo"]
    fn captura_medio_segundo_y_mide() {
        let recorder =
            Recorder::start(Source::Mic, None, None, None).expect("arrancar la captura");
        std::thread::sleep(std::time::Duration::from_millis(500));

        let status = recorder.status();
        println!(
            "{} — {} Hz, {} canales, {} muestras, rms {:.1} dB, pico {:.1} dB",
            status.device.as_deref().unwrap_or("?"),
            status.sample_rate,
            status.channels,
            status.frames,
            status.level.rms_dbfs,
            status.level.peak_dbfs
        );

        assert!(status.capturing);
        assert_eq!(status.error, None);
        assert!(
            status.frames > 0,
            "el dispositivo abrió pero no entregó ni una muestra en medio segundo"
        );
    }

    /// La cadena entera y de verdad: **el sintetizador de voz de Windows habla, el
    /// loopback lo captura, se remuestrea a 16 kHz y Silero decide que eso es voz y que
    /// ha terminado.** Es lo mas cerca que se puede estar de una entrevista sin que haya
    /// un entrevistador delante, y es automatico.
    ///
    /// Sin esto, cada pieza esta probada por separado y la cadena no lo esta, que es
    /// justo donde se esconden los fallos: un remuestreo mal alineado o una cola que no
    /// llena dan un VAD que nunca oye nada y no da ningun error.
    ///
    /// `INTERVIEW_COPILOT_VAD=<ruta> cargo test --lib -- --ignored --nocapture la_cadena`
    #[test]
    #[ignore = "habla por los altavoces y toma la salida de audio"]
    fn la_cadena_entera_detecta_una_frase_hablada() {
        let Ok(model) = std::env::var("INTERVIEW_COPILOT_VAD") else {
            panic!("define INTERVIEW_COPILOT_VAD con la ruta de silero_vad.onnx");
        };

        let recorder = Recorder::start(Source::System, None, Some(PathBuf::from(model)), None)
            .expect("abrir el loopback con VAD");

        // Una frase larga: el detector necesita al menos dos ventanas de 32 ms con voz
        // para abrir turno, y 700 ms de silencio para cerrarlo.
        let hablado = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                "Add-Type -AssemblyName System.Speech;                  $voz = New-Object System.Speech.Synthesis.SpeechSynthesizer;                  $voz.Speak('Cuéntame un proyecto complicado en el que hayas trabajado')",
            ])
            .status();
        println!("sintetizador: {hablado:?}");

        // El silencio que cierra el turno son 700 ms; se espera algo mas.
        std::thread::sleep(std::time::Duration::from_millis(1500));

        let status = recorder.status();
        let vad = status.vad.expect("la captura tenia que llevar VAD");
        println!(
            "turnos: {}, ultimo: {:?} ms, voz maxima: {:.3}, perdidas: {}",
            vad.turns, vad.last_turn_ms, vad.max_probability, vad.dropped
        );
        println!("pico que vio el VAD: {:.3}", vad.peak_in);
        println!(
            "audio: {} muestras, rms {:.1} dB, pico {:.1} dB",
            status.frames, status.level.rms_dbfs, status.level.peak_dbfs
        );

        assert_eq!(status.error, None);
        assert!(status.frames > 0, "no llegó audio por el loopback");
        assert_eq!(vad.dropped, 0, "el VAD no vio todo el audio");

        assert!(
            vad.max_probability > SPEECH_CERTAIN,
            "el sintetizador habló y el modelo no lo vio: máxima {:.3}",
            vad.max_probability
        );
        assert!(
            vad.turns >= 1,
            "hubo voz clara y no se cerró ningún turno: el fin de turno no dispara"
        );
        let hablado_ms = vad.last_turn_ms.expect("un turno cerrado trae su duracion");
        assert!(
            hablado_ms >= 1000,
            "la frase duró {hablado_ms} ms de voz: el detector la partió en trozos"
        );
    }

    /// **El hito de la fase 4, comprobado sin nadie delante.** El sintetizador de voz de
    /// Windows hace una pregunta de entrevista, el loopback la captura, el VAD cierra el
    /// turno y whisper lo transcribe. Si esto pasa, la cadena audio → texto funciona
    /// entera.
    ///
    /// `INTERVIEW_COPILOT_VAD=<onnx> INTERVIEW_COPILOT_WHISPER=<ggml> cargo test --lib -- --ignored --nocapture de_la_voz_al_texto`
    #[test]
    #[ignore = "habla por los altavoces y carga el modelo de whisper"]
    fn de_la_voz_al_texto() {
        let vad_model = std::env::var("INTERVIEW_COPILOT_VAD").expect("INTERVIEW_COPILOT_VAD");
        let whisper_model =
            std::env::var("INTERVIEW_COPILOT_WHISPER").expect("INTERVIEW_COPILOT_WHISPER");

        let transcriber =
            crate::stt::Transcriber::start(PathBuf::from(whisper_model), "whisper-base")
                .expect("arrancar la transcripcion");

        let _recorder = Recorder::start(
            Source::System,
            None,
            Some(PathBuf::from(vad_model)),
            transcriber.sender(),
        )
        .expect("abrir el loopback con VAD y transcripcion");

        let frase = "Cuentame un proyecto complicado en el que hayas trabajado";
        let hablado = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                &format!(
                    "Add-Type -AssemblyName System.Speech;                      $voz = New-Object System.Speech.Synthesis.SpeechSynthesizer;                      $voz.Speak('{frase}')"
                ),
            ])
            .status();
        println!("sintetizador: {hablado:?}");

        // 700 ms cierran el turno, mas lo que tarde whisper en cargar y transcribir. Se
        // espera comprobando en vez de dormir a ciegas: si tarda menos, el test acaba antes.
        let limite = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let texto = loop {
            let estado = transcriber.state();
            if let Some(entrada) = estado.entries.first() {
                println!(
                    "{} s de audio transcritos en {} ms: {}",
                    entrada.audio_ms / 1000,
                    entrada.took_ms,
                    entrada.text
                );
                break entrada.text.to_lowercase();
            }
            assert_eq!(estado.error, None, "la transcripcion fallo");
            assert!(
                std::time::Instant::now() < limite,
                "un minuto sin transcribir nada"
            );
            std::thread::sleep(std::time::Duration::from_millis(200));
        };

        // No se exige la frase entera: un reconocedor puede equivocarse en una palabra y
        // seguir siendo util. Se exige que haya reconocido de que se hablaba.
        assert!(
            texto.contains("proyecto") || texto.contains("complicado"),
            "transcribio algo que no se parece a lo que se dijo: {texto}"
        );
    }

    /// La misma cadena, pero **por el microfono**, que es por donde dicta el usuario.
    ///
    /// `de_la_voz_al_texto` la prueba por el loopback, y las dos fuentes no son la misma
    /// ruta: distinta lista de dispositivos, distinta configuracion —la del loopback sale
    /// de la *salida*— y ningun flujo mudo de por medio. Que una funcione no dice nada de
    /// la otra, y dictar en Entrenamiento va por esta.
    ///
    /// Hay que hablarle. Imprime lo que ve el VAD cada medio segundo para poder distinguir
    /// los tres fallos que desde fuera se ven igual —"Escuchando" y nada de texto—:
    ///
    /// - `muestras` a cero: el dispositivo abrio y no entrega audio.
    /// - `pico` a cero con muestras subiendo: el audio entra y no llega a la cola del VAD.
    /// - `pico` alto y `prob` baja: llega audio y Silero no lo toma por voz.
    ///
    /// `INTERVIEW_COPILOT_VAD=<onnx> INTERVIEW_COPILOT_WHISPER=<ggml> cargo test --lib -- --ignored --nocapture del_microfono_al_texto`
    #[test]
    #[ignore = "toma el micrófono del equipo y hay que hablarle"]
    fn del_microfono_al_texto() {
        let vad_model = std::env::var("INTERVIEW_COPILOT_VAD").expect("INTERVIEW_COPILOT_VAD");
        let whisper_model =
            std::env::var("INTERVIEW_COPILOT_WHISPER").expect("INTERVIEW_COPILOT_WHISPER");

        let transcriber =
            crate::stt::Transcriber::start(PathBuf::from(whisper_model), "whisper-base")
                .expect("arrancar la transcripcion");

        let recorder = Recorder::start(
            Source::Mic,
            None,
            Some(PathBuf::from(vad_model)),
            transcriber.sender(),
        )
        .expect("abrir el microfono con VAD y transcripcion");

        let inicial = recorder.status();
        println!(
            "microfono: {} — {} Hz, {} canales",
            inicial.device.as_deref().unwrap_or("?"),
            inicial.sample_rate,
            inicial.channels
        );
        println!("HABLA AHORA. Se mira durante 20 segundos.");

        let limite = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let mut visto: Option<String> = None;

        while std::time::Instant::now() < limite && visto.is_none() {
            std::thread::sleep(std::time::Duration::from_millis(500));

            let status = recorder.status();
            let vad = status.vad.as_ref().expect("la captura tenia que llevar VAD");
            println!(
                "muestras {:>8} · rms {:>6.1} dB · pico del VAD {:.4} · prob {:.3} (max {:.3}) · turnos {} · perdidas {}",
                status.frames,
                status.level.rms_dbfs,
                vad.peak_in,
                vad.probability,
                vad.max_probability,
                vad.turns,
                vad.dropped
            );

            let estado = transcriber.state();
            assert_eq!(estado.error, None, "la transcripcion fallo");
            if let Some(entrada) = estado.entries.first() {
                println!(
                    "TEXTO: {} ms de audio en {} ms — {}",
                    entrada.audio_ms, entrada.took_ms, entrada.text
                );
                visto = Some(entrada.text.clone());
            }
        }

        let vad = recorder.status().vad.expect("estado del VAD");
        assert!(
            vad.peak_in > 0.0,
            "el microfono abrio pero al VAD no le llego ni una muestra distinta de cero"
        );
        assert_eq!(vad.dropped, 0, "el VAD no vio todo el audio");
        assert!(
            visto.is_some(),
            "20 s sin transcribir nada: turnos cerrados {}, probabilidad maxima {:.3}",
            vad.turns,
            vad.max_probability
        );
    }

    /// El loopback, y de paso la pregunta que no se puede contestar razonando: **¿entrega
    /// datos WASAPI cuando no suena nada?** Si no lo hace, el medidor se queda congelado y
    /// hay que mantener abierto un flujo mudo de reproduccion para que siga el reloj.
    ///
    /// Mide un segundo de silencio, luego reproduce un sonido del sistema y vuelve a medir.
    ///
    /// `cargo test --lib -- --ignored --nocapture el_loopback`
    #[test]
    #[ignore = "reproduce un sonido y toma la salida de audio"]
    fn el_loopback_captura_lo_que_suena() {
        let recorder =
            Recorder::start(Source::System, None, None, None).expect("abrir el loopback");
        println!("dispositivo: {}", recorder.device);

        std::thread::sleep(std::time::Duration::from_secs(1));
        let callado = recorder.status();
        println!(
            "en silencio: {} muestras, rms {:.1} dB",
            callado.frames, callado.level.rms_dbfs
        );

        // Un WAV del propio Windows: no hace falta traer ningun fichero al repositorio.
        let sonando = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                // Barras normales: PowerShell las admite y evitan un escape que ya se colo una vez.
                "(New-Object Media.SoundPlayer 'C:/Windows/Media/Alarm01.wav').PlaySync()",
            ])
            .status();
        println!("reproducción: {sonando:?}");

        let con_sonido = recorder.status();
        println!(
            "con sonido: {} muestras, rms {:.1} dB, pico {:.1} dB",
            con_sonido.frames, con_sonido.level.rms_dbfs, con_sonido.level.peak_dbfs
        );

        assert_eq!(con_sonido.error, None);
        assert!(
            callado.frames > 0,
            "el loopback no entregó nada en un segundo de silencio: el flujo mudo no está              haciendo su trabajo y el medidor se quedaría congelado"
        );
        assert!(
            con_sonido.frames > callado.frames,
            "el loopback no entregó ni una muestra mientras sonaba un WAV"
        );
        assert!(
            con_sonido.level.peak_dbfs > -60.0,
            "sonó un WAV y el medidor no se movió: pico {:.1} dB",
            con_sonido.level.peak_dbfs
        );
    }
}
