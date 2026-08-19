//! Captura de audio y medida de nivel (§11 del spec).
//!
//! De momento solo micrófono. El loopback del sistema —el audio del entrevistador en una
//! videollamada— es el paso siguiente y lleva WASAPI aparte, asi que no hay ningun tipo
//! aqui que finja soportarlo: un `AudioSource::SystemAudio` sin implementacion detras
//! seria una promesa en un enum.

pub mod capture;
pub mod level;

pub use capture::{inputs, CaptureStatus, InputDevice, Recorder};
