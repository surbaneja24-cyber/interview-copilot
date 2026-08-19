//! Descarga de modelos, con huella comprobada.
//!
//! Vive aparte porque lo usan el VAD y whisper, y las dos precauciones de aqui no son
//! ceremonia:
//!
//! - **Se comprueba el SHA-256.** Un modelo distinto del que se probo se comporta
//!   distinto, y enterarse por un resultado raro es una tarde perdida. Ademas es lo unico
//!   que separa "he bajado un modelo" de "he ejecutado lo que habia en esa URL".
//! - **Se escribe a un fichero temporal y se renombra al final.** Una descarga cortada a
//!   la mitad dejaria un fichero que existe, no carga, y parece un fallo del codigo.
//!
//! Nada de esto ocurre solo: §2 del spec dice que la aplicacion no depende de la red, asi
//! que los modelos se descargan cuando el usuario lo pide y no al arrancar.

use std::path::{Path, PathBuf};

use crate::error::{AppError, AppResult};

/// Descarga `url` en `path` si no esta ya, comprobando su huella.
pub async fn ensure_file(url: &str, path: &Path, sha256: &str) -> AppResult<PathBuf> {
    if path.is_file() {
        return Ok(path.to_owned());
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    log::info!("descargando {url}");
    let response = reqwest::get(url)
        .await
        .map_err(|err| AppError::Invalid(format!("no se pudo descargar {url}: {err}")))?;
    if !response.status().is_success() {
        return Err(AppError::Invalid(format!(
            "la descarga de {url} respondió {}",
            response.status()
        )));
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|err| AppError::Invalid(format!("descarga interrumpida: {err}")))?;

    let digest = sha256_hex(&bytes);
    if digest != sha256 {
        return Err(AppError::Invalid(format!(
            "lo descargado no es lo que se esperaba: huella {digest}"
        )));
    }

    let partial = path.with_extension("part");
    std::fs::write(&partial, &bytes)?;
    std::fs::rename(&partial, path)?;

    log::info!("descargado {} ({} bytes)", path.display(), bytes.len());
    Ok(path.to_owned())
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write;

    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize().iter().fold(String::new(), |mut acc, byte| {
        let _ = write!(acc, "{byte:02x}");
        acc
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn la_huella_es_la_de_sha256() {
        // Vector de prueba conocido: SHA-256 de la cadena vacia.
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[tokio::test]
    async fn un_fichero_que_ya_esta_no_se_vuelve_a_descargar() {
        let dir = tempfile::tempdir().expect("directorio temporal");
        let path = dir.path().join("modelo.bin");
        std::fs::write(&path, b"ya estaba").expect("escribir");

        // La URL es basura a proposito: si intentara descargar, fallaria.
        let out = ensure_file("http://no.existe/modelo.bin", &path, "da igual")
            .await
            .expect("deberia devolver el que ya hay");

        assert_eq!(out, path);
        assert_eq!(std::fs::read(&path).expect("leer"), b"ya estaba");
    }
}
