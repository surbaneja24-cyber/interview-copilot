//! Ajustes del LLM: proveedor, modelo y parametros de generacion.
//!
//! Aqui no hay ninguna clave de API. Viven en el almacen de credenciales del sistema,
//! ver `crate::secrets`.
//!
//! **Aqui no hay ninguna clave de API.** Las claves viven en el almacen de credenciales
//! del sistema (ver `crate::secrets`), no en la base de datos, porque la base se copia,
//! se exporta y se borra entera con un boton, y una clave no debe viajar en ninguno de
//! esos tres caminos (§31).

use serde::{Deserialize, Serialize};

/// Clave bajo la que se guardan estos ajustes en la tabla `settings`.
pub const SETTINGS_KEY: &str = "llm";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// Ollama o `llama-server` en este equipo. No sale nada del ordenador.
    Local,
    /// api.openai.com. Sale la pregunta y los fragmentos recuperados.
    OpenAi,
    /// Proveedor simulado, sin IA de ninguna clase. Solo existe en compilaciones de
    /// desarrollo; ver `llm/mock.rs`.
    #[cfg(debug_assertions)]
    Mock,
}

impl ProviderKind {
    /// Proveedores que puede elegir el usuario. El simulador solo aparece en desarrollo.
    pub const SELECTABLE: &'static [Self] = &[
        Self::Local,
        Self::OpenAi,
        #[cfg(debug_assertions)]
        Self::Mock,
    ];

    /// Proveedores que guardan una clave en el almacen del sistema. Es la lista que
    /// recorre el borrado total de §15; olvidarse de anadir uno aqui deja una clave viva
    /// despues de "borrar todos mis datos".
    pub const WITH_CREDENTIALS: &'static [Self] = &[Self::OpenAi];

    /// Se enumeran todas las variantes a proposito, sin comodin: anadir un proveedor
    /// nuevo tiene que romper la compilacion aqui y obligar a decidir. Con `_ => false`,
    /// un proveedor de nube nuevo se declararia en silencio como que no saca datos del
    /// equipo, que es la peor forma posible de equivocarse en esto.
    pub fn sends_data_outside(self) -> bool {
        match self {
            Self::Local => false,
            Self::OpenAi => true,
            #[cfg(debug_assertions)]
            Self::Mock => false,
        }
    }

    /// Identificador con el que se guarda la clave en el almacen de credenciales.
    pub fn credential_id(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::OpenAi => "openai",
            #[cfg(debug_assertions)]
            Self::Mock => "mock",
        }
    }

    /// Sin comodin, por el mismo motivo que `sends_data_outside`.
    pub fn needs_api_key(self) -> bool {
        match self {
            Self::Local => false,
            Self::OpenAi => true,
            #[cfg(debug_assertions)]
            Self::Mock => false,
        }
    }

    pub fn default_base_url(self) -> &'static str {
        match self {
            // El puerto de Ollama. Con llama-server hay que cambiarlo a :8080/v1, de ahi
            // que sea configurable y no una constante escondida.
            Self::Local => "http://localhost:11434/v1",
            Self::OpenAi => "https://api.openai.com/v1",
            #[cfg(debug_assertions)]
            Self::Mock => "(ninguno)",
        }
    }

    /// Valor de partida, no una eleccion: la UI lista los modelos que el servidor declara
    /// y el usuario elige de ahi.
    pub fn default_model(self) -> &'static str {
        match self {
            // Coincide con lo que recomienda el detector de hardware para 3 GB de
            // presupuesto de memoria.
            Self::Local => "qwen2.5:3b-instruct",
            Self::OpenAi => "gpt-4o-mini",
            #[cfg(debug_assertions)]
            Self::Mock => "simulador",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmSettings {
    pub kind: ProviderKind,
    pub base_url: String,
    pub model: String,
    pub temperature: f32,
    pub max_tokens: u32,
    /// Pedir al servidor que garantice JSON. Se puede apagar para servidores que no lo
    /// soporten; el parseo no depende de ello en ningun caso.
    pub json_mode: bool,
}

impl LlmSettings {
    pub fn for_kind(kind: ProviderKind) -> Self {
        Self {
            kind,
            base_url: kind.default_base_url().to_owned(),
            model: kind.default_model().to_owned(),
            // Baja a proposito. La respuesta tiene que ceñirse a lo que dicen los
            // documentos del candidato; la creatividad aqui es exactamente el defecto
            // que §6 quiere evitar.
            temperature: 0.3,
            // La respuesta de §8 son 2-6 frases, 3-5 bullets y 2-3 preguntas, mas las
            // citas. Con 800 sobra, y el techo evita que un modelo local se enrolle
            // durante minutos.
            max_tokens: 800,
            json_mode: true,
        }
    }
}

impl Default for LlmSettings {
    /// El default del producto es LOCAL (§2 del spec), aunque el detector de hardware
    /// recomiende otra cosa en esta maquina concreta.
    fn default() -> Self {
        Self::for_kind(ProviderKind::Local)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn el_default_es_local() {
        assert_eq!(LlmSettings::default().kind, ProviderKind::Local);
        assert!(!LlmSettings::default().kind.sends_data_outside());
    }

    #[test]
    fn el_proveedor_local_no_pide_clave() {
        assert!(!ProviderKind::Local.needs_api_key());
        assert!(ProviderKind::OpenAi.needs_api_key());
    }

    /// Los ajustes se serializan a la base de datos: si el formato cambia de forma
    /// incompatible, los ajustes guardados dejan de leerse en silencio.
    #[test]
    fn los_ajustes_sobreviven_a_una_vuelta_por_json() {
        let original = LlmSettings::for_kind(ProviderKind::OpenAi);
        let json = serde_json::to_string(&original).expect("serializar");
        let recovered: LlmSettings = serde_json::from_str(&json).expect("deserializar");

        assert_eq!(recovered.kind, original.kind);
        assert_eq!(recovered.model, original.model);
        assert_eq!(recovered.base_url, original.base_url);
    }

    /// Si un proveedor pide clave y no esta en la lista del borrado total, esa clave
    /// sobrevive a "borrar todos mis datos". Es exactamente el olvido que §15 no perdona.
    #[test]
    fn todos_los_que_piden_clave_estan_en_la_lista_del_borrado_total() {
        for kind in ProviderKind::SELECTABLE {
            if kind.needs_api_key() {
                assert!(
                    ProviderKind::WITH_CREDENTIALS.contains(kind),
                    "{kind:?} guarda clave pero el borrado total no la limpia"
                );
            }
        }
    }

    /// Un descuido aqui manda la clave de OpenAI a un servidor local o al reves.
    #[test]
    fn cada_proveedor_guarda_su_clave_por_separado() {
        assert_ne!(
            ProviderKind::Local.credential_id(),
            ProviderKind::OpenAi.credential_id()
        );
    }
}
