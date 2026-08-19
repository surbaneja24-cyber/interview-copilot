use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

mod knowledge;
mod migrations;
mod settings;

pub use knowledge::{Document, DocumentKind, NewDocument, StoredChunk};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: i64,
    pub name: String,
    pub company: String,
    pub role: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewProject {
    pub name: String,
    pub company: String,
    pub role: String,
}

/// Toda la persistencia vive en un unico fichero SQLite. Un solo backup, un solo borrado,
/// y mas adelante la extension sqlite-vec vive en esta misma conexion.
pub struct Db {
    conn: Mutex<Connection>,
}

impl Db {
    pub fn open(path: &Path) -> AppResult<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(path)?;
        // WAL para que leer no bloquee escribir: durante la entrevista habra escrituras
        // de transcripcion mientras la UI lee.
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;",
        )?;

        migrations::run(&conn)?;
        log::info!("base de datos abierta en {}", path.display());

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn lock(&self) -> AppResult<MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|err| AppError::Poisoned(err.to_string()))
    }

    /// Acceso controlado a la conexion para el indice vectorial, que necesita hablar
    /// SQL directamente. El mutex sigue siendo privado: nadie fuera puede quedarse con
    /// la conexion mas alla de la llamada.
    pub fn with_conn<F, T>(&self, operation: F) -> AppResult<T>
    where
        F: FnOnce(&Connection) -> AppResult<T>,
    {
        let conn = self.lock()?;
        operation(&conn)
    }

    pub fn create_project(&self, new: &NewProject) -> AppResult<Project> {
        let name = new.name.trim();
        if name.is_empty() {
            return Err(AppError::Invalid("El proyecto necesita un nombre".into()));
        }

        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO projects (name, company, role) VALUES (?1, ?2, ?3)",
            params![name, new.company.trim(), new.role.trim()],
        )?;

        let id = conn.last_insert_rowid();
        conn.query_row(
            "SELECT id, name, company, role, created_at, updated_at
             FROM projects WHERE id = ?1",
            params![id],
            row_to_project,
        )
        .map_err(AppError::from)
    }

    pub fn list_projects(&self) -> AppResult<Vec<Project>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT id, name, company, role, created_at, updated_at
             FROM projects ORDER BY updated_at DESC",
        )?;

        let rows = stmt.query_map([], row_to_project)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
    }

    pub fn delete_project(&self, id: i64) -> AppResult<()> {
        let conn = self.lock()?;
        let affected = conn.execute("DELETE FROM projects WHERE id = ?1", params![id])?;
        if affected == 0 {
            return Err(AppError::Invalid(format!("No existe el proyecto {id}")));
        }
        Ok(())
    }

    /// §15: borrado total. Vacia las tablas de datos de usuario y compacta el fichero,
    /// para que el contenido no quede recuperable en paginas libres de la base.
    pub fn delete_all_data(&self) -> AppResult<()> {
        let conn = self.lock()?;
        for table in migrations::USER_DATA_TABLES {
            // `chunk_vectors` la crea el indice vectorial cuando se conoce la dimension
            // del modelo, no las migraciones, asi que puede no existir todavia.
            let exists: bool = conn.query_row(
                "SELECT count(*) > 0 FROM sqlite_master WHERE name = ?1",
                params![table],
                |row| row.get(0),
            )?;
            if exists {
                conn.execute_batch(&format!("DELETE FROM {table};"))?;
            }
        }
        conn.execute_batch("VACUUM;")?;
        log::warn!("borrado total de datos de usuario ejecutado");
        Ok(())
    }
}

fn row_to_project(row: &rusqlite::Row<'_>) -> rusqlite::Result<Project> {
    Ok(Project {
        id: row.get(0)?,
        name: row.get(1)?,
        company: row.get(2)?,
        role: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db() -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().expect("crear directorio temporal");
        let db = Db::open(&dir.path().join("test.db")).expect("abrir base");
        (dir, db)
    }

    fn sample() -> NewProject {
        NewProject {
            name: "Google — SWE".into(),
            company: "Google".into(),
            role: "Software Engineer".into(),
        }
    }

    #[test]
    fn crea_y_lista_proyectos() {
        let (_dir, db) = temp_db();
        let created = db.create_project(&sample()).expect("crear");
        assert_eq!(created.name, "Google — SWE");

        let all = db.list_projects().expect("listar");
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, created.id);
    }

    #[test]
    fn rechaza_nombre_vacio() {
        let (_dir, db) = temp_db();
        let empty = NewProject {
            name: "   ".into(),
            ..sample()
        };
        assert!(db.create_project(&empty).is_err());
    }

    #[test]
    fn borrado_total_deja_la_base_vacia() {
        let (_dir, db) = temp_db();
        db.create_project(&sample()).expect("crear");
        db.delete_all_data().expect("borrar todo");
        assert!(db.list_projects().expect("listar").is_empty());
    }

    #[test]
    fn las_migraciones_son_idempotentes() {
        let dir = tempfile::tempdir().expect("crear directorio temporal");
        let path = dir.path().join("test.db");
        let db = Db::open(&path).expect("primera apertura");
        db.create_project(&sample()).expect("crear");
        drop(db);

        let reopened = Db::open(&path).expect("segunda apertura");
        assert_eq!(reopened.list_projects().expect("listar").len(), 1);
    }
}
