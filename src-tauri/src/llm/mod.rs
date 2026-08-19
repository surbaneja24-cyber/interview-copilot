//! Providers de LLM (§18 del spec).
//!
//! Los tres providers —local, OpenAI y cualquiera que venga despues— hablan el mismo
//! protocolo y comparten el mismo cliente HTTP. Ni siquiera el local se enlaza dentro del
//! proceso: habla con un `llama-server` o con Ollama por HTTP. Las razones estan en
//! `docs/ARCHITECTURE.md` §2 y no se reabren aqui: cambiar de modelo sin reiniciar la
//! app, que un cuelgue del modelo no se lleve la aplicacion, y que la RAM del modelo se
//! libere sola cuando el servidor la suelta.
//!
//! Sobre el streaming: `stream_chat` es el unico metodo que cada provider implementa, y
//! `generate` se construye encima descartando los tokens intermedios. Asi la ruta con
//! streaming y la ruta sin el no pueden divergir, que es el fallo clasico de tener dos
//! implementaciones paralelas.

use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;

use crate::error::AppResult;

pub mod answer;
pub mod answering;
pub mod client;
pub mod prompt;
pub mod provider;
pub mod settings;
pub mod verify;

/// El simulador solo se compila en desarrollo. Ver la cabecera de `mock.rs`.
#[cfg(debug_assertions)]
pub mod mock;

pub use provider::HttpProvider;
pub use settings::{LlmSettings, ProviderKind};

/// Un futuro en caja. Hace falta porque `async fn` en un trait todavia no es compatible
/// con `dyn`, y aqui el provider se elige en tiempo de ejecucion desde los ajustes.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub messages: Vec<ChatMessage>,
    pub temperature: f32,
    pub max_tokens: u32,
    /// Pedir al servidor que garantice JSON. No todos los servidores lo aceptan, asi que
    /// el parseo de `answer.rs` nunca puede depender de esto: es una ayuda, no un
    /// contrato.
    pub json_mode: bool,
}

/// Lo que un provider expone de si mismo a la UI. Nunca incluye la clave de API.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderDescription {
    pub kind: ProviderKind,
    pub model: String,
    /// A donde se conecta. Se ensena al usuario para que sepa si sus datos salen del
    /// equipo (§15).
    pub endpoint: String,
    /// Si es `true`, el texto de la pregunta y los fragmentos recuperados salen del
    /// equipo. Lo decide el provider, no la UI.
    pub sends_data_outside: bool,
}

pub trait LlmProvider: Send + Sync {
    fn describe(&self) -> ProviderDescription;

    /// Modelos que ofrece el servidor. Sirve para que la UI no obligue a escribir el
    /// nombre del modelo a mano y para comprobar de un vistazo que hay servidor vivo.
    fn models(&self) -> BoxFuture<'_, AppResult<Vec<String>>>;

    /// Genera con streaming. Cada trozo de texto se envia por `tokens` segun llega; el
    /// valor devuelto es la respuesta completa concatenada.
    ///
    /// Si el receptor de `tokens` se cierra, la generacion sigue: cancelar a mitad es
    /// cosa de la capa de arriba, que es la que sabe si el usuario aborto o si solo se
    /// cerro la vista.
    fn stream_chat(
        &self,
        request: ChatRequest,
        tokens: UnboundedSender<String>,
    ) -> BoxFuture<'_, AppResult<String>>;

    /// Generacion sin streaming. Deliberadamente construida sobre `stream_chat` para que
    /// no haya dos caminos que puedan comportarse distinto.
    fn generate(&self, request: ChatRequest) -> BoxFuture<'_, AppResult<String>> {
        Box::pin(async move {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            // Vaciar el canal en paralelo no hace falta: es ilimitado, asi que el emisor
            // nunca se bloquea aunque nadie lea.
            let full = self.stream_chat(request, tx).await;
            rx.close();
            while rx.try_recv().is_ok() {}
            full
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `generate` no lo usa todavia ningun comando —la entrevista siempre va con
    /// streaming— pero es parte de la interfaz que pide §18, y esta construido sobre
    /// `stream_chat` justamente para que las dos rutas no puedan divergir. El test es lo
    /// que sostiene esa afirmacion.
    #[tokio::test]
    async fn generate_devuelve_lo_mismo_que_junta_el_streaming() {
        use crate::llm::mock::MockProvider;

        let provider = MockProvider;
        let request = ChatRequest {
            messages: vec![ChatMessage::user("FRAGMENTOS DEL CANDIDATO

[1] CV — cv
Lidere una migracion

PREGUNTA DEL ENTREVISTADOR
Cuentame algo")],
            temperature: 0.3,
            max_tokens: 800,
            json_mode: true,
        };

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let troceado = provider
            .stream_chat(request.clone(), tx)
            .await
            .expect("streaming");

        let mut juntado = String::new();
        while let Ok(piece) = rx.try_recv() {
            juntado.push_str(&piece);
        }

        let entero = provider.generate(request).await.expect("generate");

        assert_eq!(juntado, troceado, "los trozos no reconstruyen la respuesta");
        assert_eq!(entero, troceado, "generate y stream_chat no coinciden");
    }
}
