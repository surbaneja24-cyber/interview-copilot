//! Transcripcion (§3 y §18 del spec).
//!
//! Solo hay un proveedor, whisper.cpp en local, y por eso el trait tiene un unico
//! implementador de momento. Existe igualmente porque §18 lo pide y porque el proveedor de
//! nube entra en la Fase 8: lo que no se hace es inventarse ya el segundo, que seria
//! disenar contra un protocolo que nadie ha probado.

mod benchmark;
pub mod transcriber;
/// Instrumental de medida, no codigo de la aplicacion: mide lo que se equivoca una
/// transcripcion para poder comparar configuraciones con numeros. Va detras de `cfg(test)`
/// porque nada de la app lo usa, y compilar lo que nadie llama es deuda desde el dia uno.
#[cfg(test)]
pub mod wer;
pub mod whisper;

use std::path::{Path, PathBuf};

use crate::error::AppResult;

pub use transcriber::{Transcriber, TranscriptState};
pub use whisper::LocalWhisper;

/// Un modelo de whisper de los que se pueden descargar.
///
/// Las huellas son las que publica el repositorio de modelos de whisper.cpp en Hugging
/// Face. La de `base` esta ademas comprobada a mano contra el fichero descargado el
/// 2026-08-19: coincide, y eso es lo que da derecho a fiarse de las otras dos sin bajarlas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SttModel {
    /// El identificador que usa el detector de hardware al recomendar (§4).
    pub id: &'static str,
    pub file: &'static str,
    pub sha256: &'static str,
    pub bytes: u64,
}

impl SttModel {
    pub fn url(&self) -> String {
        format!("https://huggingface.co/ggerganov/whisper.cpp/resolve/main/{}", self.file)
    }

    pub fn path(&self, models_dir: &Path) -> PathBuf {
        models_dir.join(self.file)
    }

    pub fn is_downloaded(&self, models_dir: &Path) -> bool {
        self.path(models_dir).is_file()
    }

    pub async fn ensure(&self, models_dir: &Path) -> AppResult<PathBuf> {
        crate::download::ensure_file(&self.url(), &self.path(models_dir), self.sha256).await
    }
}

/// Los tres que ofrece el detector de hardware. Multilingues, no `.en`: la entrevista
/// puede ser en espanol o en ingles (§14) y un modelo solo-ingles cierra esa puerta.
pub const MODELS: &[SttModel] = &[
    SttModel {
        id: "whisper-tiny",
        file: "ggml-tiny.bin",
        sha256: "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21",
        bytes: 77_691_713,
    },
    SttModel {
        id: "whisper-base",
        file: "ggml-base.bin",
        sha256: "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe",
        bytes: 147_951_465,
    },
    SttModel {
        id: "whisper-small",
        file: "ggml-small.bin",
        sha256: "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
        bytes: 487_601_967,
    },
];

pub fn model_by_id(id: &str) -> Option<&'static SttModel> {
    MODELS.iter().find(|model| model.id == id)
}

/// §18: la transcripcion detras de un trait, para que cambiar de motor no sea reescribir
/// la aplicacion.
pub trait SttProvider: Send {
    /// Transcribe un bloque de audio de 16 kHz mono.
    ///
    /// `language` en ISO 639-1 ("es", "en"). `None` deja que el modelo lo detecte, que
    /// cuesta una pasada mas y acierta menos con frases cortas.
    fn transcribe(&mut self, samples: &[f32], language: Option<&str>) -> AppResult<String>;

    fn id(&self) -> &str;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn los_modelos_coinciden_con_los_que_recomienda_el_hardware() {
        // Si el detector recomienda un modelo que no esta aqui, la app recomienda algo
        // que no se puede descargar.
        for id in ["whisper-tiny", "whisper-base", "whisper-small"] {
            assert!(model_by_id(id).is_some(), "falta {id}");
        }
    }

    #[test]
    fn las_huellas_tienen_forma_de_sha256() {
        for model in MODELS {
            assert_eq!(model.sha256.len(), 64, "{}", model.id);
            assert!(model.sha256.chars().all(|c| c.is_ascii_hexdigit()), "{}", model.id);
        }
    }

    #[test]
    fn la_url_apunta_al_fichero_del_modelo() {
        let base = model_by_id("whisper-base").expect("base");
        assert!(base.url().ends_with("/ggml-base.bin"));
    }
}
