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
    // v4 - el material del candidato deja de pertenecer a un proyecto.
    //
    // Hasta aqui, un documento colgaba de un proyecto y cada entrevista empezaba de cero:
    // el CV, las respuestas preparadas y todo lo demas habia que volver a cargarlos. Lo
    // que el producto necesita es lo contrario — un fondo del candidato que se acumula
    // entrevista tras entrevista, y proyectos que solo aportan la oferta concreta.
    //
    // `project_id` pasa a admitir NULL, y NULL significa "es del candidato, vale para
    // todas las entrevistas". SQLite no sabe cambiar una columna, asi que la tabla se
    // reconstruye; es la receta oficial y por eso las claves ajenas se apagan durante el
    // cambio y se vuelven a encender despues.
    //
    // Se anade ademas `tag`, que para una respuesta preparada guarda el tipo de pregunta
    // que contesta (§7). Es lo que permitira entrenar por temas y detectar huecos.
    r"
    PRAGMA foreign_keys = OFF;

    CREATE TABLE documents_v4 (
        id          INTEGER PRIMARY KEY,
        project_id  INTEGER REFERENCES projects(id) ON DELETE CASCADE,
        title       TEXT NOT NULL,
        kind        TEXT NOT NULL,
        tag         TEXT,
        source_path TEXT,
        content     TEXT NOT NULL,
        created_at  TEXT NOT NULL DEFAULT (datetime('now'))
    );
    INSERT INTO documents_v4 (id, project_id, title, kind, tag, source_path, content, created_at)
        SELECT id, project_id, title, kind, NULL, source_path, content, created_at FROM documents;
    DROP TABLE documents;
    ALTER TABLE documents_v4 RENAME TO documents;
    CREATE INDEX idx_documents_project ON documents (project_id);
    CREATE INDEX idx_documents_kind ON documents (kind);

    CREATE TABLE chunks_v4 (
        id           INTEGER PRIMARY KEY,
        document_id  INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
        project_id   INTEGER REFERENCES projects(id) ON DELETE CASCADE,
        ordinal      INTEGER NOT NULL,
        text         TEXT NOT NULL,
        start_offset INTEGER NOT NULL,
        end_offset   INTEGER NOT NULL
    );
    INSERT INTO chunks_v4 (id, document_id, project_id, ordinal, text, start_offset, end_offset)
        SELECT id, document_id, project_id, ordinal, text, start_offset, end_offset FROM chunks;
    DROP TABLE chunks;
    ALTER TABLE chunks_v4 RENAME TO chunks;
    CREATE INDEX idx_chunks_document ON chunks (document_id);
    CREATE INDEX idx_chunks_project ON chunks (project_id);

    PRAGMA foreign_keys = ON;
    ",
];

pub fn run(conn: &Connection) -> AppResult<()> {
    run_to(conn, MIGRATIONS.len())
}

/// Aplica las migraciones que falten hasta `target`.
///
/// El parametro existe para los tests: es la unica forma de fabricar una base en una
/// version antigua **con el mismo SQL que se publico**, que es lo que hay que migrar de
/// verdad. Copiar el esquema viejo en el test comprobaria que la migracion funciona sobre
/// una base que nunca ha existido.
fn run_to(conn: &Connection, target: usize) -> AppResult<()> {
    let current: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let current = usize::try_from(current).unwrap_or(0);

    for (index, sql) in MIGRATIONS.iter().enumerate().take(target).skip(current) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Db;

    fn user_version(conn: &Connection) -> usize {
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("leer user_version");
        usize::try_from(version).expect("version no negativa")
    }

    /// Esquema completo tal y como lo ve SQLite: tablas, indices y su SQL.
    fn schema(conn: &Connection) -> Vec<(String, String, Option<String>)> {
        let mut stmt = conn
            .prepare(
                "SELECT type, name, sql FROM sqlite_master
                 WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name",
            )
            .expect("leer sqlite_master");
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .expect("recorrer sqlite_master");
        rows.collect::<Result<Vec<_>, _>>().expect("filas")
    }

    /// El caso que de verdad importa: una base que ya existe en el equipo de alguien, con
    /// sus datos dentro, y que al abrir la version nueva de la app tiene que migrarse sin
    /// perder nada. Una migracion que falla aqui no da error: deja la app medio rota y no
    /// se nota hasta mucho despues.
    #[test]
    fn una_base_en_v2_con_datos_llega_a_la_ultima_version_sin_perderlos() {
        let dir = tempfile::tempdir().expect("directorio temporal");
        let path = dir.path().join("vieja.db");

        {
            let conn = Connection::open(&path).expect("crear la base vieja");
            conn.execute_batch("PRAGMA foreign_keys = ON;")
                .expect("claves ajenas");
            run_to(&conn, 2).expect("dejarla en v2");
            assert_eq!(user_version(&conn), 2);
            assert!(
                conn.execute_batch("SELECT 1 FROM settings;").is_err(),
                "en v2 la tabla settings no existe todavia: si existe, esta base no es v2"
            );

            conn.execute_batch(
                "INSERT INTO projects (id, name, company, role)
                 VALUES (1, 'Supply Rodamientos — Mozo', 'Supply Rodamientos', 'Mozo de almacén');
                 INSERT INTO documents (id, project_id, title, kind, content)
                 VALUES (1, 1, 'cv.pdf', 'cv', 'Carnet de carretillero en vigor.');
                 INSERT INTO chunks (document_id, project_id, ordinal, text, start_offset, end_offset)
                 VALUES (1, 1, 0, 'Carnet de carretillero en vigor.', 0, 32);
                 INSERT INTO index_meta (id, model_id, dimensions)
                 VALUES (1, 'multilingual-e5-base', 768);",
            )
            .expect("meter datos de usuario");
        }

        let db = Db::open(&path).expect("abrir con la version nueva");

        let version = db
            .with_conn(|conn| Ok(user_version(conn)))
            .expect("leer version");
        assert_eq!(version, MIGRATIONS.len(), "la base no llego a la ultima version");

        let projects = db.list_projects().expect("listar proyectos");
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].company, "Supply Rodamientos");

        let documents = db.list_documents(1).expect("listar documentos");
        assert_eq!(documents.len(), 1);
        assert_eq!(
            documents[0].chunk_count, 1,
            "los trozos indexados no sobrevivieron a la migracion"
        );
        assert_eq!(
            db.index_model().expect("modelo del indice"),
            Some(("multilingual-e5-base".to_owned(), 768)),
            "la base perdio con que modelo se construyo el indice: se reindexaria entero"
        );

        // Y lo que traen las versiones nuevas tiene que funcionar, no solo existir.
        db.save_settings("prueba", &"valor").expect("guardar ajuste");
        assert_eq!(
            db.load_settings::<String>("prueba").expect("leer ajuste"),
            Some("valor".to_owned())
        );

        // v4: un documento sin proyecto, que es como se guarda el material del candidato.
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO documents (project_id, title, kind, tag, content)
                 VALUES (NULL, '¿Tu mayor fracaso?', 'prepared_answers', 'failure', 'Pues...')",
                [],
            )?;
            Ok(())
        })
        .expect("un documento del candidato tiene que caber sin proyecto");
    }

    /// Migrar desde cualquier version tiene que dejar exactamente el mismo esquema que
    /// crear la base de cero. Recorre todas las versiones, asi que una migracion nueva
    /// que se olvide de un indice rompe este test sin tener que acordarse de ampliarlo.
    #[test]
    fn una_base_vieja_acaba_igual_que_una_recien_creada() {
        let nueva = Connection::open_in_memory().expect("base en memoria");
        run(&nueva).expect("migrar de cero");
        let esperado = schema(&nueva);

        for desde in 0..MIGRATIONS.len() {
            let vieja = Connection::open_in_memory().expect("base en memoria");
            run_to(&vieja, desde).expect("dejarla en una version antigua");
            run(&vieja).expect("migrar hasta la ultima");

            assert_eq!(user_version(&vieja), MIGRATIONS.len());
            assert_eq!(
                schema(&vieja),
                esperado,
                "una base que venia de v{desde} no acabo con el esquema de una nueva"
            );
        }
    }

    /// Volver a migrar una base que ya esta al dia no puede tocar nada. Es lo que ocurre
    /// en cada arranque de la aplicacion.
    #[test]
    fn migrar_dos_veces_no_cambia_nada() {
        let conn = Connection::open_in_memory().expect("base en memoria");
        run(&conn).expect("primera pasada");
        let antes = schema(&conn);
        run(&conn).expect("segunda pasada");
        assert_eq!(schema(&conn), antes);
        assert_eq!(user_version(&conn), MIGRATIONS.len());
    }
}
