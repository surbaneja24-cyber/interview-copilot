//! El provider concreto que habla con un servidor compatible con OpenAI.
//!
//! Hay uno solo, y no uno por proveedor, porque los tres hablan el mismo protocolo byte
//! a byte. Lo unico que difiere de verdad —si pide clave, si los datos salen del
//! equipo— vive en `ProviderKind`. Ver `docs/ARCHITECTURE.md` seccion 2.
//!
//! **Por que hay una sola implementacion y no una por proveedor.** El spec (§18) nombra
//! `LocalLLMProvider` y `OpenAIProvider` como piezas separadas, y en la mayoria de
//! arquitecturas lo serian. Aqui no: Ollama y `llama-server` exponen exactamente la API
//! de chat de OpenAI, byte a byte. Dos structs serian dos copias del mismo codigo
//! diferenciadas por una URL y una cabecera, que es justo lo que §23 prohibe.
//!
//! Lo que si esta separado es lo unico que de verdad difiere: si el proveedor pide clave
//! y si los datos salen del equipo. Eso vive en `ProviderKind`.
//!
//! El dia que entre un proveedor con otro protocolo —Anthropic, por ejemplo, que tiene
//! otra forma de mensajes y otro streaming— tendra su propio struct implementando el
//! mismo trait. Ese es el momento en que la separacion aporta algo.

use tokio::sync::mpsc::UnboundedSender;

use crate::error::{AppError, AppResult};
use crate::llm::client::{Endpoint, OpenAiCompatibleClient};
use crate::llm::settings::{LlmSettings, ProviderKind};
use crate::llm::{BoxFuture, ChatRequest, LlmProvider, ProviderDescription};

pub struct HttpProvider {
    kind: ProviderKind,
    endpoint: Endpoint,
    client: OpenAiCompatibleClient,
}

impl HttpProvider {
    /// Construye el provider que digan los ajustes.
    ///
    /// `api_key` la trae quien llama desde el almacen de credenciales; este modulo nunca
    /// la lee ni la guarda, solo la manda en la cabecera.
    pub fn from_settings(settings: &LlmSettings, api_key: Option<String>) -> AppResult<Self> {
        if settings.kind.needs_api_key() && api_key.as_ref().is_none_or(|key| key.trim().is_empty())
        {
            return Err(AppError::Invalid(format!(
                "Falta la clave de API de {}. Configurala en Ajustes.",
                settings.kind.credential_id()
            )));
        }

        let base_url = settings.base_url.trim();
        if base_url.is_empty() {
            return Err(AppError::Invalid("La URL del servidor esta vacia".into()));
        }
        if settings.model.trim().is_empty() {
            return Err(AppError::Invalid("No hay ningun modelo elegido".into()));
        }

        Ok(Self {
            kind: settings.kind,
            endpoint: Endpoint {
                base_url: base_url.to_owned(),
                model: settings.model.trim().to_owned(),
                // El servidor local no lleva autenticacion: mandarle una cabecera con una
                // clave de OpenAI seria filtrarla a un proceso que no la necesita.
                api_key: api_key.filter(|_| settings.kind.needs_api_key()),
            },
            client: OpenAiCompatibleClient::new()?,
        })
    }
}

impl LlmProvider for HttpProvider {
    fn describe(&self) -> ProviderDescription {
        ProviderDescription {
            kind: self.kind,
            model: self.endpoint.model.clone(),
            endpoint: self.endpoint.base_url.clone(),
            sends_data_outside: self.kind.sends_data_outside(),
        }
    }

    fn models(&self) -> BoxFuture<'_, AppResult<Vec<String>>> {
        Box::pin(async move { self.client.models(&self.endpoint).await })
    }

    fn stream_chat(
        &self,
        request: ChatRequest,
        tokens: UnboundedSender<String>,
    ) -> BoxFuture<'_, AppResult<String>> {
        Box::pin(async move {
            self.client
                .stream_chat(&self.endpoint, &request, &tokens)
                .await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn el_proveedor_local_se_construye_sin_clave() {
        let settings = LlmSettings::for_kind(ProviderKind::Local);
        let provider = HttpProvider::from_settings(&settings, None).expect("construir");
        assert!(!provider.describe().sends_data_outside);
    }

    #[test]
    fn openai_sin_clave_falla_antes_de_salir_a_la_red() {
        let settings = LlmSettings::for_kind(ProviderKind::OpenAi);
        let Err(err) = HttpProvider::from_settings(&settings, None) else {
            panic!("openai sin clave deberia fallar");
        };
        assert!(err.to_string().contains("clave"));
    }

    /// Una clave de espacios no es una clave. Sin esto, la peticion sale a la red para
    /// volver con un 401 que el usuario no sabe interpretar.
    #[test]
    fn una_clave_en_blanco_cuenta_como_ausente() {
        let settings = LlmSettings::for_kind(ProviderKind::OpenAi);
        assert!(HttpProvider::from_settings(&settings, Some("   ".into())).is_err());
    }

    /// Que la clave de un proveedor de nube no acabe viajando a un servidor local por
    /// haber cambiado de proveedor sin limpiar el campo.
    #[test]
    fn la_clave_no_se_adjunta_a_un_servidor_local() {
        let settings = LlmSettings::for_kind(ProviderKind::Local);
        let provider =
            HttpProvider::from_settings(&settings, Some("sk-secreta".into())).expect("construir");
        assert!(provider.endpoint.api_key.is_none());
    }

    #[test]
    fn rechaza_ajustes_incompletos() {
        let mut settings = LlmSettings::for_kind(ProviderKind::Local);
        settings.model = "  ".into();
        assert!(HttpProvider::from_settings(&settings, None).is_err());

        let mut settings = LlmSettings::for_kind(ProviderKind::Local);
        settings.base_url = String::new();
        assert!(HttpProvider::from_settings(&settings, None).is_err());
    }
}
