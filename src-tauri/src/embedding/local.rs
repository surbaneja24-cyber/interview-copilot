//! Embeddings locales con fastembed (ONNX Runtime, sin Python).
//!
//! El modelo concreto no se elige aqui por intuicion: ver `benchmark.rs`, que compara
//! candidatos sobre un corpus de entrevista real y deja la medicion por escrito.
//!
//! Restriccion de uso: una sola instancia por proceso. Dos hilos inicializando el mismo
//! modelo a la vez compiten por el directorio de cache y uno de los dos falla con
//! "Failed to retrieve onnx/model.onnx". La aplicacion crea el proveedor una vez y lo
//! comparte como estado de Tauri.

use std::path::Path;
use std::sync::Mutex;

use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};

use super::EmbeddingProvider;
use crate::error::{AppError, AppResult};

/// Todo lo que distingue a un modelo de otro. Los prefijos importan: la familia E5 se
/// entreno con ellos y omitirlos degrada la recuperacion de forma silenciosa, mientras
/// que los modelos de la familia paraphrase no los usan y anadirlos los empeora.
pub struct ModelSpec {
    pub id: &'static str,
    pub model: EmbeddingModel,
    pub dimensions: usize,
    pub document_prefix: &'static str,
    pub query_prefix: &'static str,
    /// Tamano aproximado en disco, medido tras descargarlo. `fastembed` no expone el
    /// progreso de descarga, asi que la UI compara los bytes que van cayendo en la carpeta
    /// de modelos contra esta cifra para mostrar un avance real.
    pub approx_bytes: u64,
}

/// Multilingue, sin cuantizar. 470 MB en disco.
pub const MULTILINGUAL_E5_SMALL: ModelSpec = ModelSpec {
    id: "multilingual-e5-small",
    model: EmbeddingModel::MultilingualE5Small,
    dimensions: 384,
    document_prefix: "passage: ",
    query_prefix: "query: ",
    approx_bytes: 488_000_000,
};

/// Multilingue cuantizado, bastante mas pequeno. Candidato para hardware ajustado.
pub const PARAPHRASE_ML_MINILM_Q: ModelSpec = ModelSpec {
    id: "paraphrase-multilingual-minilm-l12-v2-q",
    model: EmbeddingModel::ParaphraseMLMiniLML12V2Q,
    dimensions: 384,
    document_prefix: "",
    query_prefix: "",
    approx_bytes: 252_000_000,
};

/// Multilingue, un escalon por encima del small. ~1,1 GB en disco.
pub const MULTILINGUAL_E5_BASE: ModelSpec = ModelSpec {
    id: "multilingual-e5-base",
    model: EmbeddingModel::MultilingualE5Base,
    dimensions: 768,
    document_prefix: "passage: ",
    query_prefix: "query: ",
    approx_bytes: 1_127_000_000,
};

/// Multilingue de la familia sentence-transformers, sin prefijos. ~1 GB en disco.
pub const PARAPHRASE_ML_MPNET: ModelSpec = ModelSpec {
    id: "paraphrase-multilingual-mpnet-base-v2",
    model: EmbeddingModel::ParaphraseMLMpnetBaseV2,
    dimensions: 768,
    document_prefix: "",
    query_prefix: "",
    approx_bytes: 1_127_000_000,
};

/// Solo para el banco de pruebas: el mismo E5 sin los prefijos, para medir cuanto
/// aportan de verdad en vez de darlo por supuesto.
#[cfg(test)]
pub const E5_SMALL_SIN_PREFIJOS: ModelSpec = ModelSpec {
    id: "multilingual-e5-small (sin prefijos)",
    model: EmbeddingModel::MultilingualE5Small,
    dimensions: 384,
    document_prefix: "",
    query_prefix: "",
    approx_bytes: 488_000_000,
};

/// Modelo por defecto de la aplicacion.
///
/// Elegido por medicion, no por intuicion: en `benchmark.rs` acierta 6 de 6 preguntas
/// mientras que el small acierta 2 y el mpnet —del mismo tamano— acierta 3. Lo que
/// decide no es el tamano sino el objetivo de entrenamiento: E5 esta entrenado para
/// recuperacion asimetrica (pregunta corta contra parrafo largo), que es exactamente
/// esta tarea.
pub const DEFAULT_MODEL: &ModelSpec = &MULTILINGUAL_E5_BASE;

pub struct LocalEmbeddingProvider {
    spec: &'static ModelSpec,
    // TextEmbedding no es Sync: la sesion de ONNX se serializa con un mutex. No es un
    // cuello de botella porque el indexado ocurre al preparar la entrevista, no durante.
    model: Mutex<TextEmbedding>,
}

impl LocalEmbeddingProvider {
    pub fn new(cache_dir: &Path) -> AppResult<Self> {
        Self::with_model(cache_dir, DEFAULT_MODEL)
    }

    /// Descarga el modelo la primera vez y lo cachea en `cache_dir`.
    pub fn with_model(cache_dir: &Path, spec: &'static ModelSpec) -> AppResult<Self> {
        std::fs::create_dir_all(cache_dir)?;

        let options = InitOptions::new(spec.model.clone())
            .with_cache_dir(cache_dir.to_path_buf())
            .with_show_download_progress(true);

        let model = TextEmbedding::try_new(options)
            .map_err(|err| AppError::Invalid(format!("no se pudo cargar {}: {err}", spec.id)))?;

        log::info!(
            "modelo de embeddings {} listo en {}",
            spec.id,
            cache_dir.display()
        );

        Ok(Self {
            spec,
            model: Mutex::new(model),
        })
    }

    fn run(&self, texts: Vec<String>) -> AppResult<Vec<Vec<f32>>> {
        let model = self
            .model
            .lock()
            .map_err(|err| AppError::Poisoned(err.to_string()))?;

        model
            .embed(texts, None)
            .map_err(|err| AppError::Invalid(format!("fallo al generar embeddings: {err}")))
    }
}

impl EmbeddingProvider for LocalEmbeddingProvider {
    fn embed_documents(&self, texts: &[String]) -> AppResult<Vec<Vec<f32>>> {
        let prefixed = texts
            .iter()
            .map(|text| format!("{}{text}", self.spec.document_prefix))
            .collect();
        self.run(prefixed)
    }

    fn embed_query(&self, text: &str) -> AppResult<Vec<f32>> {
        let mut result = self.run(vec![format!("{}{text}", self.spec.query_prefix)])?;
        result
            .pop()
            .ok_or_else(|| AppError::Invalid("el modelo no devolvio ningun vector".into()))
    }

    fn dimensions(&self) -> usize {
        self.spec.dimensions
    }

    fn id(&self) -> &str {
        self.spec.id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider() -> LocalEmbeddingProvider {
        let cache = std::env::temp_dir().join("interview-copilot-models");
        LocalEmbeddingProvider::new(&cache).expect("cargar el modelo")
    }

    /// La primera ejecucion descarga el modelo. Se ignora por defecto para no meter una
    /// descarga en cada `cargo test`; se corre con
    /// `cargo test --lib -- --ignored --nocapture --test-threads=1`.
    ///
    /// El `--test-threads=1` no es opcional: ver la nota de arriba del modulo.
    #[test]
    #[ignore = "descarga el modelo"]
    fn genera_vectores_de_la_dimension_esperada() {
        let provider = provider();
        let vectors = provider
            .embed_documents(&["hola mundo".to_owned(), "adios mundo".to_owned()])
            .expect("embeder");

        assert_eq!(vectors.len(), 2);
        assert_eq!(vectors[0].len(), provider.dimensions());
    }
}
