//! Captura de audio y medida de nivel (§11 del spec).
//!
//! Dos fuentes: el microfono (la voz del usuario) y el audio del sistema por loopback de
//! WASAPI (la voz del entrevistador en la videollamada). Se capturan por separado a
//! proposito: separar por fuente es lo que distingue quien habla en el MVP 1, sin tener
//! que reconocer voces.

pub mod capture;
pub mod level;

pub use capture::{devices, CaptureStatus, InputDevice, Recorder, Source};
