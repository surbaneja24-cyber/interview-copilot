//! Indice vectorial sobre sqlite-vec.
//!
//! sqlite-vec se registra como extension automatica de SQLite antes de abrir ninguna
//! conexion, asi que las tablas `vec0` estan disponibles en la misma base que el resto
//! de los datos. Una sola base, un solo backup, un solo borrado (§15).

use std::sync::Once;

use rusqlite::{params, Connection};

use crate::error::{AppError, AppResult};

static REGISTER_EXTENSION: Once = Once::new();

/// Registra sqlite-vec para todas las conexiones que se abran despues.
///
/// Se llama una sola vez por proceso; `Once` lo garantiza incluso si dos hilos abren la
/// base a la vez.
pub fn register() {
    REGISTER_EXTENSION.call_once(|| {
        // SAFETY: sqlite3_auto_extension espera un puntero a funcion de inicializacion
        // de extension. `sqlite3_vec_init` tiene esa firma; el transmute solo ajusta el
        // tipo de puntero, que es como la propia documentacion de sqlite-vec indica
        // registrarla desde Rust.
        unsafe {
            rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
                sqlite_vec::sqlite3_vec_init as *const (),
            )));
        }
        log::info!("extension sqlite-vec registrada");
    });
}

/// Serializa un embedding al formato que espera sqlite-vec: f32 en little endian.
pub fn encode(vector: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(vector.len() * 4);
    for value in vector {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

/// Crea la tabla virtual del indice si no existe. `dimensions` tiene que coincidir con
/// la salida del modelo de embeddings; cambiar de modelo obliga a reindexar.
pub fn create_index(conn: &Connection, dimensions: usize) -> AppResult<()> {
    conn.execute_batch(&format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS chunk_vectors USING vec0(
            chunk_id INTEGER PRIMARY KEY,
            embedding FLOAT[{dimensions}]
        );"
    ))?;
    Ok(())
}

/// Reindexa un chunk. Las tablas virtuales de sqlite-vec no admiten `ON CONFLICT`
/// ("UPSERT not implemented for virtual table"), asi que se borra y se vuelve a
/// insertar. Las dos sentencias van juntas para que no quede un hueco sin vector si
/// falla la segunda.
pub fn upsert(conn: &Connection, chunk_id: i64, embedding: &[f32]) -> AppResult<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "DELETE FROM chunk_vectors WHERE chunk_id = ?1",
        params![chunk_id],
    )?;
    tx.execute(
        "INSERT INTO chunk_vectors (chunk_id, embedding) VALUES (?1, ?2)",
        params![chunk_id, encode(embedding)],
    )?;
    tx.commit()?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Match {
    pub chunk_id: i64,
    /// Distancia L2. Cuanto menor, mas parecido.
    pub distance: f64,
}

pub fn search(conn: &Connection, embedding: &[f32], limit: usize) -> AppResult<Vec<Match>> {
    let mut stmt = conn.prepare(
        "SELECT chunk_id, distance FROM chunk_vectors
         WHERE embedding MATCH ?1 AND k = ?2
         ORDER BY distance",
    )?;

    let rows = stmt.query_map(params![encode(embedding), limit as i64], |row| {
        Ok(Match {
            chunk_id: row.get(0)?,
            distance: row.get(1)?,
        })
    })?;

    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Connection {
        register();
        let conn = Connection::open_in_memory().expect("abrir base en memoria");
        create_index(&conn, 3).expect("crear indice");
        conn
    }

    #[test]
    fn la_extension_esta_disponible() {
        register();
        let conn = Connection::open_in_memory().expect("abrir base en memoria");
        let version: String = conn
            .query_row("SELECT vec_version()", [], |row| row.get(0))
            .expect("vec_version() deberia existir si la extension cargo");
        println!("sqlite-vec version: {version}");
        assert!(!version.is_empty());
    }

    #[test]
    fn encuentra_el_vecino_mas_cercano() {
        let conn = store();
        upsert(&conn, 1, &[1.0, 0.0, 0.0]).expect("insertar 1");
        upsert(&conn, 2, &[0.0, 1.0, 0.0]).expect("insertar 2");
        upsert(&conn, 3, &[0.9, 0.1, 0.0]).expect("insertar 3");

        let hits = search(&conn, &[1.0, 0.0, 0.0], 2).expect("buscar");
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].chunk_id, 1);
        assert_eq!(hits[1].chunk_id, 3);
        assert!(hits[0].distance < hits[1].distance);
    }

    #[test]
    fn reindexar_sustituye_el_vector_anterior() {
        let conn = store();
        upsert(&conn, 1, &[1.0, 0.0, 0.0]).expect("insertar");
        upsert(&conn, 1, &[0.0, 0.0, 1.0]).expect("reindexar");

        let hits = search(&conn, &[0.0, 0.0, 1.0], 1).expect("buscar");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].chunk_id, 1);
        assert!(hits[0].distance < 0.001, "deberia ser practicamente exacto");
    }

    #[test]
    fn codifica_en_little_endian() {
        assert_eq!(encode(&[1.0f32]), 1.0f32.to_le_bytes().to_vec());
        assert_eq!(encode(&[1.0, 2.0]).len(), 8);
    }
}
