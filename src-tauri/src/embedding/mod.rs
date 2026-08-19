//! Generacion de embeddings, detras de una interfaz (§18 del spec).
//!
//! Cambiar de modelo cambia la dimension del vector y obliga a reindexar todo el corpus,
//! asi que el proveedor expone su identificador: quien guarda el indice puede detectar
//! que fue construido con otro modelo y rehacerlo en vez de mezclar vectores
//! incomparables.

use crate::error::AppResult;

mod benchmark;
mod local;

pub use local::{LocalEmbeddingProvider, ModelSpec, DEFAULT_MODEL};

pub trait EmbeddingProvider: Send + Sync {
    /// Indexa documentos. Algunos modelos distinguen entre indexar y consultar, de ahi
    /// que haya dos metodos y no uno.
    fn embed_documents(&self, texts: &[String]) -> AppResult<Vec<Vec<f32>>>;

    /// Convierte la pregunta del entrevistador en vector para buscar en el indice.
    fn embed_query(&self, text: &str) -> AppResult<Vec<f32>>;

    fn dimensions(&self) -> usize;

    /// Identificador estable del modelo, para guardarlo junto al indice.
    fn id(&self) -> &str;
}
