//! Claves de API, fuera de la base de datos (§31 del spec).
//!
//! Se guardan en el almacen de credenciales del sistema —en Windows, el Administrador de
//! credenciales— y no en el fichero SQLite. La razon no es esoterica: ese fichero se
//! copia, se puede adjuntar en un informe de errores y se borra entero con el boton de
//! §15. Una clave que viaje por cualquiera de esos tres caminos es una clave filtrada.
//!
//! Regla que no se rompe: **no existe ningun comando que devuelva la clave al frontend.**
//! La UI solo puede preguntar si hay una configurada, ponerla o borrarla. §31 dice
//! explicitamente que no se muestren claves en la interfaz, y la unica forma de
//! garantizarlo es que no haya por donde pedirlas.

use crate::error::{AppError, AppResult};

/// Identificador de la aplicacion dentro del almacen. Coincide con el de `tauri.conf.json`
/// para que las credenciales sean reconocibles si el usuario abre el Administrador de
/// credenciales de Windows y se pregunta que es esto.
const SERVICE: &str = "dev.urbaneja.interviewcopilot";

fn entry(provider: &str) -> AppResult<keyring::Entry> {
    keyring::Entry::new(SERVICE, provider)
        .map_err(|err| AppError::Secrets(format!("no se pudo abrir el almacen: {err}")))
}

pub fn store(provider: &str, key: &str) -> AppResult<()> {
    let key = key.trim();
    if key.is_empty() {
        return Err(AppError::Invalid("La clave esta vacia".into()));
    }

    entry(provider)?
        .set_password(key)
        .map_err(|err| AppError::Secrets(format!("no se pudo guardar la clave: {err}")))?;

    log::info!("clave de {provider} guardada en el almacen del sistema");
    Ok(())
}

/// Lee la clave. Solo la usa el backend para montar la cabecera de autenticacion; nunca
/// cruza hacia el frontend.
pub fn read(provider: &str) -> AppResult<Option<String>> {
    match entry(provider)?.get_password() {
        Ok(key) => Ok(Some(key)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(err) => Err(AppError::Secrets(format!(
            "no se pudo leer la clave: {err}"
        ))),
    }
}

pub fn has(provider: &str) -> bool {
    matches!(read(provider), Ok(Some(_)))
}

/// Borra la clave. Que no hubiera ninguna no es un error: el resultado que se pedia
/// —que no quede clave guardada— se cumple igual.
pub fn clear(provider: &str) -> AppResult<()> {
    match entry(provider)?.delete_credential() {
        Ok(()) => {
            log::info!("clave de {provider} borrada del almacen del sistema");
            Ok(())
        }
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(err) => Err(AppError::Secrets(format!(
            "no se pudo borrar la clave: {err}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn una_clave_vacia_no_se_guarda() {
        assert!(store("test-vacia", "   ").is_err());
    }

    /// Toca el almacen real del sistema, asi que usa un identificador propio y limpia
    /// detras. Va marcado como ignorado para que la bateria normal no dependa de que haya
    /// un almacen de credenciales disponible.
    #[test]
    #[ignore = "escribe en el almacen de credenciales del sistema"]
    fn guarda_lee_y_borra() {
        let provider = "interview-copilot-test";

        store(provider, "sk-de-prueba").expect("guardar");
        assert_eq!(read(provider).expect("leer"), Some("sk-de-prueba".into()));
        assert!(has(provider));

        clear(provider).expect("borrar");
        assert_eq!(read(provider).expect("leer"), None);

        // Borrar dos veces no falla.
        clear(provider).expect("borrar de nuevo");
    }
}
