//! Captura de micrófono con cpal (WASAPI en Windows).
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

use std::str::FromStr;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{DeviceId, FromSample, Sample, SizedSample};

use crate::audio::level::{Level, Meter};
use crate::error::{AppError, AppResult};

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
}

impl CaptureStatus {
    pub fn idle() -> Self {
        Self {
            capturing: false,
            device: None,
            sample_rate: 0,
            channels: 0,
            level: Level::SILENT,
            frames: 0,
            error: None,
        }
    }
}

pub fn inputs() -> AppResult<Vec<InputDevice>> {
    let host = cpal::default_host();
    let default_id = host
        .default_input_device()
        .and_then(|device| device.id().ok());

    let devices = host
        .input_devices()
        .map_err(|err| AppError::Audio(format!("no se pudieron listar los micrófonos: {err}")))?;

    let mut out = Vec::new();
    for device in devices {
        // Un dispositivo que no sabe decir ni quien es ni con que configuracion abre no
        // se puede ofrecer, pero tampoco puede tumbar la lista de los demas.
        let (Ok(id), Ok(description), Ok(config)) = (
            device.id(),
            device.description(),
            device.default_input_config(),
        ) else {
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

pub struct Recorder {
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
    pub fn start(requested: Option<String>) -> AppResult<Self> {
        let meter = Arc::new(Meter::new());
        let failure: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let (ready_tx, ready_rx) = mpsc::channel::<AppResult<Opened>>();

        let thread_meter = Arc::clone(&meter);
        let thread_failure = Arc::clone(&failure);

        let thread = std::thread::Builder::new()
            .name("captura-audio".into())
            .spawn(move || {
                match open_stream(requested.as_deref(), &thread_meter, &thread_failure) {
                    Err(err) => {
                        let _ = ready_tx.send(Err(err));
                    }
                    Ok((stream, opened)) => {
                        if let Err(err) = stream.play() {
                            let _ = ready_tx.send(Err(AppError::Audio(format!(
                                "el dispositivo abrió pero no arrancó: {err}"
                            ))));
                            return;
                        }

                        let _ = ready_tx.send(Ok(opened));
                        // Bloquea hasta que se suelte el emisor: es el hilo duenno del
                        // stream y tiene que seguir vivo mientras haya captura.
                        let _ = stop_rx.recv();
                        drop(stream);
                    }
                }
            })
            .map_err(|err| AppError::Audio(format!("no se pudo crear el hilo de audio: {err}")))?;

        let opened = ready_rx
            .recv()
            .map_err(|_| AppError::Audio("el hilo de audio murió al arrancar".into()))??;

        log::info!(
            "capturando de \"{}\" a {} Hz, {} canales",
            opened.device,
            opened.sample_rate,
            opened.channels
        );

        Ok(Self {
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

fn open_stream(
    requested: Option<&str>,
    meter: &Arc<Meter>,
    failure: &Arc<Mutex<Option<String>>>,
) -> AppResult<(cpal::Stream, Opened)> {
    let host = cpal::default_host();

    let device = match requested {
        Some(id) => {
            let id = DeviceId::from_str(id)
                .map_err(|err| AppError::Audio(format!("identificador de micrófono ilegible: {err}")))?;
            host.device_by_id(&id).ok_or_else(|| {
                AppError::Audio("ese micrófono ya no está conectado".to_owned())
            })?
        }
        None => host
            .default_input_device()
            .ok_or_else(|| AppError::Audio("este equipo no tiene micrófono".into()))?,
    };

    let name = device
        .description()
        .map(|description| description.name().to_owned())
        .map_err(|err| AppError::Audio(format!("el dispositivo no dice su nombre: {err}")))?;
    let config = device.default_input_config().map_err(|err| {
        AppError::Audio(format!("no se pudo leer la configuración de \"{name}\": {err}"))
    })?;

    let opened = Opened {
        device: name.clone(),
        sample_rate: config.sample_rate(),
        channels: config.channels(),
    };

    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => build::<f32>(&device, &config.into(), meter, failure),
        cpal::SampleFormat::I16 => build::<i16>(&device, &config.into(), meter, failure),
        cpal::SampleFormat::U16 => build::<u16>(&device, &config.into(), meter, failure),
        // Se enumeran los tres que WASAPI entrega en la practica. Un formato nuevo tiene
        // que dar un error visible, no una captura muda.
        other => {
            return Err(AppError::Audio(format!(
                "formato de muestra no soportado: {other}"
            )))
        }
    }?;

    Ok((stream, opened))
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    meter: &Arc<Meter>,
    failure: &Arc<Mutex<Option<String>>>,
) -> AppResult<cpal::Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let meter = Arc::clone(meter);
    let on_error = Arc::clone(failure);
    let mut buffer: Vec<f32> = Vec::new();

    device
        .build_input_stream(
            config,
            move |data: &[T], _| {
                // El buffer vive en la clausura y se reutiliza: reservar memoria dentro
                // de la llamada de retorno de audio es lo que produce cortes.
                buffer.clear();
                buffer.extend(data.iter().map(|sample| f32::from_sample(*sample)));
                meter.push(&buffer);
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

    /// No comprueba que haya micrófono —una máquina sin tarjeta de sonido es legitima—
    /// sino que enumerar no revienta y que lo que sale tiene sentido.
    #[test]
    fn enumerar_no_revienta() {
        let devices = inputs().expect("enumerar dispositivos");
        println!("{} dispositivos de entrada", devices.len());

        for device in &devices {
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
            devices.iter().filter(|device| device.is_default).count() <= 1,
            "no puede haber dos dispositivos por defecto"
        );
    }

    /// Abre y cierra de verdad. Va marcado `#[ignore]` porque toma el micrófono del equipo
    /// y en una máquina sin entrada de audio fallaria por el motivo equivocado.
    ///
    /// `cargo test --lib -- --ignored --nocapture captura_medio_segundo`
    #[test]
    #[ignore = "toma el micrófono del equipo"]
    fn captura_medio_segundo_y_mide() {
        let recorder = Recorder::start(None).expect("arrancar la captura");
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
}
