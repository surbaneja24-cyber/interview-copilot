//! Ajustes persistentes: clave y valor, con el valor en JSON.
//!
//! Nunca guarda claves de API. Ver `crate::secrets`.
//!
//! Nunca guarda claves de API. Ver `crate::secrets` y §31.

use rusqlite::{params, OptionalExtension};
use serde::de::DeserializeOwned;
use serde::Serialize;

use super::Db;
use crate::error::{AppError, AppResult};

impl Db {
    /// Lee unos ajustes guardados. Devuelve `None` tanto si no hay nada guardado como si
    /// lo guardado ya no encaja con la forma actual del tipo: en ambos casos lo correcto
    /// es volver a los valores por defecto, no romper el arranque de la aplicacion.
    pub fn load_settings<T: DeserializeOwned>(&self, key: &str) -> AppResult<Option<T>> {
        let conn = self.lock()?;
        let raw: Option<String> = conn
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()?;

        let Some(raw) = raw else {
            return Ok(None);
        };

        match serde_json::from_str(&raw) {
            Ok(value) => Ok(Some(value)),
            Err(err) => {
                log::warn!("ajustes '{key}' ilegibles, se usan los por defecto: {err}");
                Ok(None)
            }
        }
    }

    pub fn save_settings<T: Serialize>(&self, key: &str, value: &T) -> AppResult<()> {
        let raw = serde_json::to_string(value)
            .map_err(|err| AppError::Invalid(format!("ajustes no serializables: {err}")))?;

        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, datetime('now'))
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            params![key, raw],
        )?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::llm::{LlmSettings, ProviderKind};
    use crate::storage::Db;

    fn temp_db() -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().expect("crear directorio temporal");
        let db = Db::open(&dir.path().join("test.db")).expect("abrir base");
        (dir, db)
    }

    #[test]
    fn sin_nada_guardado_no_hay_ajustes() {
        let (_dir, db) = temp_db();
        let loaded: Option<LlmSettings> = db.load_settings("llm").expect("leer");
        assert!(loaded.is_none());
    }

    #[test]
    fn guarda_y_recupera() {
        let (_dir, db) = temp_db();
        let mut settings = LlmSettings::for_kind(ProviderKind::OpenAi);
        settings.model = "gpt-4o".into();

        db.save_settings("llm", &settings).expect("guardar");
        let loaded: LlmSettings = db.load_settings("llm").expect("leer").expect("hay ajustes");

        assert_eq!(loaded.kind, ProviderKind::OpenAi);
        assert_eq!(loaded.model, "gpt-4o");
    }

    #[test]
    fn guardar_dos_veces_sustituye_en_vez_de_duplicar() {
        let (_dir, db) = temp_db();
        db.save_settings("llm", &LlmSettings::for_kind(ProviderKind::Local))
            .expect("primera");
        db.save_settings("llm", &LlmSettings::for_kind(ProviderKind::OpenAi))
            .expect("segunda");

        let loaded: LlmSettings = db.load_settings("llm").expect("leer").expect("hay ajustes");
        assert_eq!(loaded.kind, ProviderKind::OpenAi);
    }

    /// Unos ajustes de una version anterior que ya no encajan no pueden impedir que la
    /// aplicacion arranque: se ignoran y se vuelve a los valores por defecto.
    #[test]
    fn unos_ajustes_ilegibles_no_rompen_el_arranque() {
        let (_dir, db) = temp_db();
        db.save_settings("llm", &"esto no es un LlmSettings")
            .expect("guardar basura");

        let loaded: Option<LlmSettings> = db.load_settings("llm").expect("leer");
        assert!(loaded.is_none());
    }

    /// §15: el borrado total tiene que llevarse tambien los ajustes.
    #[test]
    fn el_borrado_total_se_lleva_los_ajustes() {
        let (_dir, db) = temp_db();
        db.save_settings("llm", &LlmSettings::default())
            .expect("guardar");
        db.delete_all_data().expect("borrar todo");

        let loaded: Option<LlmSettings> = db.load_settings("llm").expect("leer");
        assert!(loaded.is_none());
    }
}
