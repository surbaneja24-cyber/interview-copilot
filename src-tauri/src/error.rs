use serde::{Serialize, Serializer};

/// Error unico del backend. Se serializa como string plano porque al otro lado hay
/// TypeScript: el frontend nunca debe ramificar sobre variantes internas de Rust.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("error de base de datos: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("error de entrada/salida: {0}")]
    Io(#[from] std::io::Error),

    #[error("error de Tauri: {0}")]
    Tauri(#[from] tauri::Error),

    /// El mutex de la conexion se envenena si un hilo entra en panic mientras la tiene.
    #[error("estado interno corrupto: {0}")]
    Poisoned(String),

    /// Fallo hablando con un proveedor de LLM. Lleva dentro el mensaje del servidor:
    /// un "401" a secas no le dice nada a nadie.
    #[error("{0}")]
    Provider(String),

    /// El almacen de credenciales del sistema no responde.
    #[error("{0}")]
    Secrets(String),

    /// Captura de audio: dispositivo que no abre, que desaparece o formato imposible.
    #[error("{0}")]
    Audio(String),

    #[error("{0}")]
    Invalid(String),
}

impl Serialize for AppError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

pub type AppResult<T> = Result<T, AppError>;
