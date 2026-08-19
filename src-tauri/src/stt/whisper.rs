//! whisper.cpp en local, a traves de `whisper-rs`.
//!
//! Es la pieza mas cara de la aplicacion y la que decide si el modo LOCAL es usable: en la
//! maquina de referencia son 4 nucleos Zen 2 y 5,7 GB, con el modelo de embeddings y el
//! LLM peleando por lo mismo. Por eso el contexto se carga bajo peticion y se suelta al
//! parar, igual que el de embeddings.

use std::path::Path;

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::error::{AppError, AppResult};
use crate::stt::SttProvider;

/// Hilos que se le dan a whisper.
///
/// Uno menos que los nucleos logicos, y nunca menos de uno: durante la entrevista esto
/// corre a la vez que la captura de audio y el VAD, y dejarle todos los hilos al
/// transcriptor produce cortes en el audio, que es el unico dato que no se puede recuperar
/// despues.
fn threads() -> i32 {
    let cores = std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(2);
    i32::try_from(cores.saturating_sub(1).max(1)).unwrap_or(1)
}

pub struct LocalWhisper {
    id: String,
    context: WhisperContext,
    threads: i32,
}

impl LocalWhisper {
    pub fn load(model: &Path, id: &str) -> AppResult<Self> {
        // whisper.cpp escribe su diagnostico en stdout, y esta aplicacion no tiene consola:
        // ese texto se pierde. Esto lo manda al mismo `log` que todo lo demas, una sola vez.
        static HOOKS: std::sync::Once = std::sync::Once::new();
        HOOKS.call_once(whisper_rs::install_logging_hooks);

        let path = model
            .to_str()
            .ok_or_else(|| AppError::Invalid("la ruta del modelo no es texto válido".into()))?;

        let context = WhisperContext::new_with_params(path, WhisperContextParameters::default())
            .map_err(|err| AppError::Invalid(format!("no se pudo cargar {id}: {err}")))?;

        log::info!("whisper cargado: {id}, {} hilos", threads());

        Ok(Self {
            id: id.to_owned(),
            context,
            threads: threads(),
        })
    }
}

impl SttProvider for LocalWhisper {
    fn transcribe(&mut self, samples: &[f32], language: Option<&str>) -> AppResult<String> {
        // whisper trabaja con ventanas de 30 s. Un turno mas largo que eso hay que
        // trocearlo, y eso llega con la transcripcion incremental; hasta entonces, mejor
        // decirlo que devolver media frase en silencio.
        if samples.len() > 30 * 16_000 {
            return Err(AppError::Invalid(
                "el turno pasa de 30 s y todavía no se trocea".into(),
            ));
        }

        let mut state = self
            .context
            .create_state()
            .map_err(|err| AppError::Invalid(format!("whisper no arrancó: {err}")))?;

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(self.threads);
        params.set_language(language);
        // Nada de esto va a una consola: la aplicacion no tiene una, y whisper.cpp escribe
        // en stdout por defecto.
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        // La entrevista es del usuario, no una clase de idiomas: traducir al ingles lo que
        // se dice en espanol seria cambiar la pregunta antes de responderla.
        params.set_translate(false);

        state
            .full(params, samples)
            .map_err(|err| AppError::Invalid(format!("whisper falló transcribiendo: {err}")))?;

        let mut text = String::new();
        for segment in state.as_iter() {
            let piece = segment
                .to_str_lossy()
                .map_err(|err| AppError::Invalid(format!("segmento ilegible: {err}")))?;
            text.push_str(&piece);
        }

        Ok(text.trim().to_owned())
    }

    fn id(&self) -> &str {
        &self.id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deja_al_menos_un_hilo_libre() {
        let hilos = threads();
        assert!(hilos >= 1);
        let cores = std::thread::available_parallelism()
            .map(std::num::NonZeroUsize::get)
            .unwrap_or(2);
        assert!(
            (hilos as usize) < cores || cores == 1,
            "whisper se quedaria con todos los nucleos y la captura de audio daria cortes"
        );
    }

    /// El modelo de verdad sobre voz de verdad. Genera el WAV con el sintetizador de
    /// Windows (16 kHz, mono, 16 bits) y pasa las dos rutas:
    ///
    /// `INTERVIEW_COPILOT_WHISPER=<ggml-base.bin> INTERVIEW_COPILOT_WAV=<wav> cargo test --lib -- --ignored --nocapture transcribe_una_frase`
    #[test]
    #[ignore = "carga el modelo de whisper y tarda"]
    fn transcribe_una_frase() {
        let model = std::env::var("INTERVIEW_COPILOT_WHISPER").expect("INTERVIEW_COPILOT_WHISPER");
        let wav = std::env::var("INTERVIEW_COPILOT_WAV").expect("INTERVIEW_COPILOT_WAV");

        let bytes = std::fs::read(&wav).expect("leer el wav");
        let data = bytes
            .windows(4)
            .position(|w| w == b"data")
            .map(|pos| pos + 8)
            .expect("el wav no trae bloque data");
        let muestras: Vec<f32> = bytes[data..]
            .chunks_exact(2)
            .map(|par| i16::from_le_bytes([par[0], par[1]]) as f32 / 32768.0)
            .collect();

        let empezo = std::time::Instant::now();
        let mut whisper = LocalWhisper::load(Path::new(&model), "whisper-base").expect("cargar");
        let cargado = empezo.elapsed();

        let empezo = std::time::Instant::now();
        let texto = whisper.transcribe(&muestras, Some("es")).expect("transcribir");
        let tardo = empezo.elapsed();

        let audio_s = muestras.len() as f32 / 16_000.0;
        println!("carga: {cargado:.1?}");
        println!("audio: {audio_s:.1} s, transcripción: {tardo:.1?}");
        println!("texto: {texto}");

        assert!(!texto.is_empty(), "no transcribió nada");
    }
}
