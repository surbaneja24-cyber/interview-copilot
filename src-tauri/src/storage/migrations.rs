use rusqlite::Connection;

use crate::error::AppResult;

/// Cada entrada es una version del esquema. Nunca se edita una migracion ya publicada:
/// se anade una nueva al final. `user_version` de SQLite guarda en que version esta el
/// fichero, asi que no hace falta tabla de control propia.
const MIGRATIONS: &[&str] = &[
    // v1 — proyectos (§20 del spec)
    r"
    CREATE TABLE projects (
        id          INTEGER PRIMARY KEY,
        name        TEXT NOT NULL,
        company     TEXT NOT NULL,
        role        TEXT NOT NULL,
        created_at  TEXT NOT NULL DEFAULT (datetime('now')),
        updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
    );
    CREATE INDEX idx_projects_updated ON projects (updated_at DESC);
    ",
    // v2 — base de conocimiento: documentos y sus trozos indexables (§5 del spec)
    r"
    CREATE TABLE documents (
        id          INTEGER PRIMARY KEY,
        project_id  INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
        title       TEXT NOT NULL,
        kind        TEXT NOT NULL,
        source_path TEXT,
        content     TEXT NOT NULL,
        created_at  TEXT NOT NULL DEFAULT (datetime('now'))
    );
    CREATE INDEX idx_documents_project ON documents (project_id);

    CREATE TABLE chunks (
        id           INTEGER PRIMARY KEY,
        document_id  INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
        project_id   INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
        ordinal      INTEGER NOT NULL,
        text         TEXT NOT NULL,
        start_offset INTEGER NOT NULL,
        end_offset   INTEGER NOT NULL
    );
    CREATE INDEX idx_chunks_document ON chunks (document_id);
    CREATE INDEX idx_chunks_project ON chunks (project_id);

    -- Con que modelo se construyo el indice. Cambiar de modelo cambia la dimension del
    -- vector y hace incomparables los embeddings viejos con los nuevos: guardarlo es lo
    -- que permite detectarlo y reindexar en vez de devolver resultados sin sentido.
    CREATE TABLE index_meta (
        id             INTEGER PRIMARY KEY CHECK (id = 1),
        model_id       TEXT NOT NULL,
        dimensions     INTEGER NOT NULL,
        indexed_at     TEXT NOT NULL DEFAULT (datetime('now'))
    );
    ",
    // v3 - ajustes persistentes (§19). Clave/valor con el valor en JSON: los ajustes
    // cambian de forma con cada fase y una columna por opcion obligaria a migrar el
    // esquema cada vez.
    //
    // Aqui NO va ninguna clave de API: viven en el almacen de credenciales del sistema
    // (ver `crate::secrets`). Esta tabla se copia y se exporta; una clave no debe.
    r"
    CREATE TABLE settings (
        key        TEXT PRIMARY KEY,
        value      TEXT NOT NULL,
        updated_at TEXT NOT NULL DEFAULT (datetime('now'))
    );
    ",
];

pub fn run(conn: &Connection) -> AppResult<()> {
    let current: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let current = usize::try_from(current).unwrap_or(0);

    for (index, sql) in MIGRATIONS.iter().enumerate().skip(current) {
        let version = index + 1;
        log::info!("aplicando migracion v{version}");
        conn.execute_batch(sql)?;
        // pragma_update no admite parametros enlazados para user_version.
        conn.execute_batch(&format!("PRAGMA user_version = {version};"))?;
    }

    Ok(())
}

/// Tablas que borra `delete_all_data`. Se declara aparte de las migraciones para que
/// anadir una tabla nueva sin anadirla aqui sea un olvido visible en el diff (§15).
/// El orden importa: los hijos antes que los padres, porque `VACUUM` no arregla una
/// violacion de clave ajena a medio camino.
pub const USER_DATA_TABLES: &[&str] = &[
    "chunk_vectors",
    "chunks",
    "documents",
    "index_meta",
    "settings",
    "projects",
];
