//! Cliente HTTP compartido por todos los providers.
//!
//! Ollama, `llama-server` y api.openai.com exponen la misma API de chat, asi que hay un
//! solo cliente y un solo parseo de Server-Sent Events.
//!
//! Los tres hablan la API de chat de OpenAI: Ollama y `llama-server` la exponen igual que
//! api.openai.com. Un solo cliente, un solo parseo de SSE, un solo sitio donde arreglar
//! los errores de red.

use std::time::Duration;

use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedSender;

use crate::error::{AppError, AppResult};
use crate::llm::{ChatRequest, Role};

/// Cuanto se espera entre trozos de respuesta. Un modelo local recien arrancado tiene que
/// cargar los pesos desde disco, que en esta maquina son decenas de segundos, y eso no es
/// un cuelgue.
const READ_TIMEOUT: Duration = Duration::from_secs(120);

/// Configuracion de un extremo compatible con OpenAI.
#[derive(Debug, Clone)]
pub struct Endpoint {
    /// Sin barra final y sin `/chat/completions`: por ejemplo
    /// `https://api.openai.com/v1` o `http://localhost:11434/v1`.
    pub base_url: String,
    pub model: String,
    /// `None` en los servidores locales, que no piden autenticacion.
    pub api_key: Option<String>,
}

impl Endpoint {
    fn url(&self, path: &str) -> String {
        format!("{}/{path}", self.base_url.trim_end_matches('/'))
    }
}

pub struct OpenAiCompatibleClient {
    http: reqwest::Client,
}

impl OpenAiCompatibleClient {
    pub fn new() -> AppResult<Self> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(READ_TIMEOUT)
            .build()
            .map_err(|err| {
                AppError::Provider(format!("no se pudo crear el cliente HTTP: {err}"))
            })?;

        Ok(Self { http })
    }

    pub async fn models(&self, endpoint: &Endpoint) -> AppResult<Vec<String>> {
        let response = self
            .authorized(self.http.get(endpoint.url("models")), endpoint)
            .send()
            .await
            .map_err(|err| connection_error(endpoint, &err))?;

        let body = read_json(response, endpoint).await?;

        #[derive(Deserialize)]
        struct ModelList {
            data: Vec<Model>,
        }
        #[derive(Deserialize)]
        struct Model {
            id: String,
        }

        let list: ModelList = serde_json::from_value(body)
            .map_err(|err| AppError::Provider(format!("lista de modelos ilegible: {err}")))?;

        let mut ids: Vec<String> = list.data.into_iter().map(|model| model.id).collect();
        ids.sort();
        Ok(ids)
    }

    /// Envia la peticion y va emitiendo por `tokens` cada trozo de texto que llega.
    /// Devuelve la respuesta completa.
    pub async fn stream_chat(
        &self,
        endpoint: &Endpoint,
        request: &ChatRequest,
        tokens: &UnboundedSender<String>,
    ) -> AppResult<String> {
        let response = match self.post_chat(endpoint, request, request.json_mode).await {
            Ok(response) => response,
            // Algunos servidores locales rechazan `response_format`. Es un fallo de
            // capacidad, no de la peticion: se reintenta sin el en vez de dejar al
            // usuario delante de un 400 que no puede interpretar.
            Err(AppError::Provider(message))
                if request.json_mode && message.contains("response_format") =>
            {
                log::warn!("el servidor no acepta response_format, se reintenta sin el");
                self.post_chat(endpoint, request, false).await?
            }
            Err(other) => return Err(other),
        };

        let mut stream = response.bytes_stream();
        let mut pending = String::new();
        let mut full = String::new();

        while let Some(chunk) = stream.next().await {
            let bytes = chunk.map_err(|err| {
                AppError::Provider(format!("la conexion se corto a mitad de respuesta: {err}"))
            })?;
            pending.push_str(&String::from_utf8_lossy(&bytes));

            // Solo se procesan las lineas completas: el ultimo trozo de cada lectura casi
            // siempre llega partido por la mitad.
            while let Some(cut) = pending.find('\n') {
                let line: String = pending.drain(..=cut).collect();
                match parse_sse_line(line.trim_end()) {
                    SseLine::Ignore => {}
                    SseLine::Done => return finish(full),
                    SseLine::Data(payload) => {
                        if let Some(text) = delta_text(&payload)? {
                            full.push_str(&text);
                            // Que nadie escuche no es un error: la capa de arriba pudo
                            // cerrar la vista. La respuesta completa se sigue montando.
                            let _ = tokens.send(text);
                        }
                    }
                }
            }
        }

        finish(full)
    }

    async fn post_chat(
        &self,
        endpoint: &Endpoint,
        request: &ChatRequest,
        json_mode: bool,
    ) -> AppResult<reqwest::Response> {
        let messages: Vec<Value> = request
            .messages
            .iter()
            .map(|message| {
                json!({
                    "role": match message.role {
                        Role::System => "system",
                        Role::User => "user",
                        Role::Assistant => "assistant",
                    },
                    "content": message.content,
                })
            })
            .collect();

        let mut body = json!({
            "model": endpoint.model,
            "messages": messages,
            "temperature": request.temperature,
            "max_tokens": request.max_tokens,
            "stream": true,
        });

        if json_mode {
            body["response_format"] = json!({ "type": "json_object" });
        }

        let response = self
            .authorized(self.http.post(endpoint.url("chat/completions")), endpoint)
            .json(&body)
            .send()
            .await
            .map_err(|err| connection_error(endpoint, &err))?;

        ensure_success(response, endpoint).await
    }

    fn authorized(
        &self,
        builder: reqwest::RequestBuilder,
        endpoint: &Endpoint,
    ) -> reqwest::RequestBuilder {
        match endpoint.api_key.as_deref() {
            Some(key) => builder.bearer_auth(key),
            None => builder,
        }
    }
}

/// Un servidor que cierra la conexion sin mandar `[DONE]` pero que ya emitio texto no es
/// un error. Uno que cierra sin emitir nada, si.
fn finish(full: String) -> AppResult<String> {
    if full.is_empty() {
        return Err(AppError::Provider(
            "el modelo no devolvio ningun texto".into(),
        ));
    }
    Ok(full)
}

fn connection_error(endpoint: &Endpoint, err: &reqwest::Error) -> AppError {
    if err.is_connect() {
        AppError::Provider(format!(
            "no hay nadie escuchando en {}. Si es un modelo local, arranca Ollama o \
             llama-server antes.",
            endpoint.base_url
        ))
    } else if err.is_timeout() {
        AppError::Provider(format!(
            "{} no respondio a tiempo. Un modelo local recien arrancado puede tardar en \
             cargar los pesos.",
            endpoint.base_url
        ))
    } else {
        AppError::Provider(format!("fallo hablando con {}: {err}", endpoint.base_url))
    }
}

/// Convierte una respuesta de error en un `AppError` con el mensaje del servidor dentro.
/// Sin esto, una clave caducada llega a la UI como "error 401" y nadie sabe que hacer.
async fn ensure_success(
    response: reqwest::Response,
    endpoint: &Endpoint,
) -> AppResult<reqwest::Response> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }

    let body = response.text().await.unwrap_or_default();
    let detail = error_detail(&body);

    Err(AppError::Provider(format!(
        "{} respondio {status}: {detail}{}",
        endpoint.base_url,
        hint_for(status.as_u16(), &detail),
    )))
}

/// Los servidores compatibles con OpenAI meten el motivo real en `error.message`. Si no
/// viene asi, se ensena el cuerpo recortado antes que nada.
fn error_detail(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .pointer("/error/message")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| body.chars().take(300).collect())
}

/// Anade una pista solo cuando aporta algo que el servidor no ha dicho ya.
///
/// El 429 es el caso que obliga a mirar el cuerpo: significa tanto "vas demasiado rapido"
/// como "la cuenta no tiene saldo", y son dos problemas sin nada que ver. Dar por hecho
/// el primero le decia al usuario que esperase cuando lo que tenia que hacer era pagar,
/// contradiciendo ademas al propio mensaje del servidor justo encima.
fn hint_for(status: u16, detail: &str) -> &'static str {
    let detail = detail.to_lowercase();

    match status {
        401 | 403 => " Revisa la clave de API en Ajustes.",
        404 => " Revisa la URL del servidor y que el modelo exista.",
        429 if mentions_billing(&detail) => {
            " No es un limite de ritmo: la cuenta se ha quedado sin saldo."
        }
        429 => " Estas enviando peticiones mas rapido de lo que el proveedor admite.",
        _ => "",
    }
}

fn mentions_billing(detail: &str) -> bool {
    ["credit", "quota", "billing", "saldo", "payment"]
        .iter()
        .any(|word| detail.contains(word))
}

async fn read_json(response: reqwest::Response, endpoint: &Endpoint) -> AppResult<Value> {
    ensure_success(response, endpoint)
        .await?
        .json::<Value>()
        .await
        .map_err(|err| AppError::Provider(format!("respuesta ilegible: {err}")))
}

enum SseLine {
    Data(Value),
    Done,
    Ignore,
}

/// Una linea de Server-Sent Events. Solo interesan las de `data:`; los comentarios que
/// mandan algunos servidores para mantener viva la conexion empiezan por dos puntos.
fn parse_sse_line(line: &str) -> SseLine {
    let Some(payload) = line.strip_prefix("data:") else {
        return SseLine::Ignore;
    };

    let payload = payload.trim();
    if payload == "[DONE]" {
        return SseLine::Done;
    }
    if payload.is_empty() {
        return SseLine::Ignore;
    }

    match serde_json::from_str::<Value>(payload) {
        Ok(value) => SseLine::Data(value),
        // Un JSON partido no deberia llegar aqui —las lineas se procesan completas— pero
        // si llega, tirarlo es mejor que abortar toda la respuesta.
        Err(err) => {
            log::warn!("evento SSE ilegible, se descarta: {err}");
            SseLine::Ignore
        }
    }
}

/// Saca el texto de un evento de streaming. Devuelve `None` cuando el evento no lleva
/// texto, que es lo normal en el primero (solo trae el rol) y en el ultimo (solo el
/// motivo de parada).
fn delta_text(event: &Value) -> AppResult<Option<String>> {
    if let Some(message) = event.pointer("/error/message").and_then(Value::as_str) {
        return Err(AppError::Provider(message.to_owned()));
    }

    let text = event
        .pointer("/choices/0/delta/content")
        .and_then(Value::as_str);

    Ok(text.filter(|text| !text.is_empty()).map(str::to_owned))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconoce_el_final_del_stream() {
        assert!(matches!(parse_sse_line("data: [DONE]"), SseLine::Done));
    }

    #[test]
    fn ignora_comentarios_y_lineas_sin_datos() {
        assert!(matches!(parse_sse_line(": keep-alive"), SseLine::Ignore));
        assert!(matches!(parse_sse_line(""), SseLine::Ignore));
        assert!(matches!(parse_sse_line("event: message"), SseLine::Ignore));
    }

    #[test]
    fn extrae_el_texto_de_un_delta() {
        let event = json!({ "choices": [{ "delta": { "content": "Hola" } }] });
        assert_eq!(delta_text(&event).expect("delta"), Some("Hola".to_owned()));
    }

    /// El primer evento solo trae el rol y el ultimo solo el motivo de parada. Ninguno de
    /// los dos es un error ni debe colar una cadena vacia en la respuesta.
    #[test]
    fn los_eventos_sin_texto_no_aportan_nada() {
        let solo_rol = json!({ "choices": [{ "delta": { "role": "assistant" } }] });
        assert_eq!(delta_text(&solo_rol).expect("delta"), None);

        let cierre = json!({ "choices": [{ "delta": {}, "finish_reason": "stop" }] });
        assert_eq!(delta_text(&cierre).expect("delta"), None);

        let vacio = json!({ "choices": [{ "delta": { "content": "" } }] });
        assert_eq!(delta_text(&vacio).expect("delta"), None);
    }

    /// Algunos servidores mandan el error dentro del propio stream, con codigo 200. Si no
    /// se mira, la respuesta sale vacia y sin explicacion.
    #[test]
    fn un_error_dentro_del_stream_se_propaga() {
        let event = json!({ "error": { "message": "modelo no cargado" } });
        let err = delta_text(&event).expect_err("deberia ser error");
        assert!(err.to_string().contains("modelo no cargado"));
    }

    #[test]
    fn la_url_se_compone_sin_barras_duplicadas() {
        let endpoint = Endpoint {
            base_url: "http://localhost:11434/v1/".into(),
            model: "qwen2.5:3b".into(),
            api_key: None,
        };
        assert_eq!(
            endpoint.url("chat/completions"),
            "http://localhost:11434/v1/chat/completions"
        );
    }

    #[test]
    fn el_detalle_del_error_sale_del_cuerpo_json() {
        let body = r#"{"error":{"message":"model not found","type":"invalid_request_error"}}"#;
        assert_eq!(error_detail(body), "model not found");
    }

    /// Un 429 por falta de saldo y uno por ritmo son problemas distintos y la solucion no
    /// se parece en nada. Confundirlos manda al usuario a esperar cuando tiene que pagar.
    #[test]
    fn distingue_el_429_por_saldo_del_429_por_ritmo() {
        let sin_saldo = hint_for(429, "You have no credits remaining. Add credits to continue");
        assert!(sin_saldo.contains("saldo"));
        assert!(!sin_saldo.contains("rapido"));

        let por_ritmo = hint_for(429, "Rate limit reached for gpt-4o-mini");
        assert!(por_ritmo.contains("rapido"));
        assert!(!por_ritmo.contains("saldo"));
    }

    #[test]
    fn los_codigos_sin_pista_util_no_anaden_nada() {
        assert_eq!(hint_for(500, "internal server error"), "");
        assert_eq!(hint_for(503, "overloaded"), "");
    }

    /// Ollama y llama-server no siempre devuelven el sobre de OpenAI. El cuerpo crudo es
    /// peor mensaje, pero es infinitamente mejor que uno vacio.
    #[test]
    fn un_error_sin_formato_conocido_ensena_el_cuerpo() {
        assert_eq!(error_detail("model requires more system memory"), "model requires more system memory");
    }
}
