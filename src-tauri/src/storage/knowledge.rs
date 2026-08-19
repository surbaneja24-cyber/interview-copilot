//! Persistencia de la base de conocimiento: documentos y sus trozos indexables.

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use super::Db;
use crate::error::{AppError, AppResult};
use crate::rag::chunking::Chunk;

/// De donde sale el texto. Sirve para pesar la recuperacion mas adelante: lo que dice el
/// CV del candidato no vale lo mismo que lo que dice la oferta de la empresa.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentKind {
    Cv,
    JobOffer,
    Company,
    Notes,
    PreparedAnswers,
    Other,
}

impl DocumentKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Cv => "cv",
            Self::JobOffer => "job_offer",
            Self::Company => "company",
            Self::Notes => "notes",
            Self::PreparedAnswers => "prepared_answers",
            Self::Other => "other",
        }
    }

    fn parse(raw: &str) -> Self {
        match raw {
            "cv" => Self::Cv,
            "job_offer" => Self::JobOffer,
            "company" => Self::Company,
            "notes" => Self::Notes,
            "prepared_answers" => Self::PreparedAnswers,
            _ => Self::Other,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Document {
    pub id: i64,
    pub project_id: i64,
    pub title: String,
    pub kind: DocumentKind,
    pub source_path: Option<String>,
    pub created_at: String,
    /// Cuantos trozos indexables produjo. Cero significa que el documento entro pero no
    /// se ha indexado, que es un estado que la UI tiene que poder mostrar.
    pub chunk_count: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewDocument {
    pub project_id: i64,
    pub title: String,
    pub kind: DocumentKind,
    pub source_path: Option<String>,
    pub content: String,
}

/// Un trozo tal y como sale de la base, con su procedencia.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredChunk {
    pub id: i64,
    pub document_id: i64,
    pub document_title: String,
    pub kind: DocumentKind,
    pub ordinal: i64,
    pub text: String,
}

impl Db {
    pub fn create_document(&self, new: &NewDocument) -> AppResult<Document> {
        let title = new.title.trim();
        if title.is_empty() {
            return Err(AppError::Invalid("El documento necesita un titulo".into()));
        }
        if new.content.trim().is_empty() {
            return Err(AppError::Invalid(format!(
                "No se pudo extraer texto de \"{title}\""
            )));
        }

        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO documents (project_id, title, kind, source_path, content)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                new.project_id,
                title,
                new.kind.as_str(),
                new.source_path,
                new.content
            ],
        )?;

        let id = conn.last_insert_rowid();
        read_document(&conn, id)
    }

    pub fn list_documents(&self, project_id: i64) -> AppResult<Vec<Document>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT d.id, d.project_id, d.title, d.kind, d.source_path, d.created_at,
                    (SELECT count(*) FROM chunks c WHERE c.document_id = d.id)
             FROM documents d
             WHERE d.project_id = ?1
             ORDER BY d.created_at DESC",
        )?;

        let rows = stmt.query_map(params![project_id], row_to_document)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
    }

    pub fn document_content(&self, document_id: i64) -> AppResult<String> {
        let conn = self.lock()?;
        conn.query_row(
            "SELECT content FROM documents WHERE id = ?1",
            params![document_id],
            |row| row.get(0),
        )
        .map_err(AppError::from)
    }

    pub fn delete_document(&self, document_id: i64) -> AppResult<()> {
        let conn = self.lock()?;
        // Los trozos caen por ON DELETE CASCADE; sus vectores hay que quitarlos a mano
        // porque una tabla virtual de sqlite-vec no participa en las claves ajenas.
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM chunk_vectors
             WHERE chunk_id IN (SELECT id FROM chunks WHERE document_id = ?1)",
            params![document_id],
        )
        .or_else(ignore_missing_vector_table)?;
        tx.execute("DELETE FROM documents WHERE id = ?1", params![document_id])?;
        tx.commit()?;
        Ok(())
    }

    /// Sustituye los trozos de un documento. Devuelve los identificadores asignados, en
    /// el mismo orden que los trozos recibidos, para poder asociarlos a sus vectores.
    pub fn replace_chunks(
        &self,
        document_id: i64,
        project_id: i64,
        chunks: &[Chunk],
    ) -> AppResult<Vec<i64>> {
        let conn = self.lock()?;
        let tx = conn.unchecked_transaction()?;

        tx.execute(
            "DELETE FROM chunk_vectors
             WHERE chunk_id IN (SELECT id FROM chunks WHERE document_id = ?1)",
            params![document_id],
        )
        .or_else(ignore_missing_vector_table)?;
        tx.execute(
            "DELETE FROM chunks WHERE document_id = ?1",
            params![document_id],
        )?;

        let mut ids = Vec::with_capacity(chunks.len());
        {
            let mut stmt = tx.prepare(
                "INSERT INTO chunks (document_id, project_id, ordinal, text, start_offset, end_offset)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for (ordinal, chunk) in chunks.iter().enumerate() {
                stmt.insert(params![
                    document_id,
                    project_id,
                    ordinal as i64,
                    chunk.text,
                    chunk.start as i64,
                    chunk.end as i64
                ])
                .map(|id| ids.push(id))?;
            }
        }

        tx.commit()?;
        Ok(ids)
    }

    /// Recupera trozos por identificador, conservando el orden pedido: el buscador ya los
    /// ordeno por relevancia y ese orden no se puede perder al hidratarlos.
    pub fn chunks_by_id(&self, ids: &[i64]) -> AppResult<Vec<StoredChunk>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }

        let conn = self.lock()?;
        let placeholders = vec!["?"; ids.len()].join(",");
        let mut stmt = conn.prepare(&format!(
            "SELECT c.id, c.document_id, d.title, d.kind, c.ordinal, c.text
             FROM chunks c
             JOIN documents d ON d.id = c.document_id
             WHERE c.id IN ({placeholders})"
        ))?;

        let rows = stmt.query_map(rusqlite::params_from_iter(ids), |row| {
            Ok(StoredChunk {
                id: row.get(0)?,
                document_id: row.get(1)?,
                document_title: row.get(2)?,
                kind: DocumentKind::parse(&row.get::<_, String>(3)?),
                ordinal: row.get(4)?,
                text: row.get(5)?,
            })
        })?;

        let mut found: Vec<StoredChunk> = rows.collect::<Result<Vec<_>, _>>()?;
        found.sort_by_key(|chunk| {
            ids.iter()
                .position(|id| *id == chunk.id)
                .unwrap_or(usize::MAX)
        });
        Ok(found)
    }

    /// Trozos de un proyecto que todavia no tienen vector.
    pub fn unindexed_chunks(&self, project_id: i64) -> AppResult<Vec<(i64, String)>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT c.id, c.text FROM chunks c
             WHERE c.project_id = ?1
               AND c.id NOT IN (SELECT chunk_id FROM chunk_vectors)
             ORDER BY c.id",
        )?;

        let rows = stmt.query_map(params![project_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
    }

    /// Modelo con el que se construyo el indice, si hay alguno.
    pub fn index_model(&self) -> AppResult<Option<(String, usize)>> {
        let conn = self.lock()?;
        let found = conn
            .query_row(
                "SELECT model_id, dimensions FROM index_meta WHERE id = 1",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .ok();

        Ok(found.map(|(id, dims)| (id, dims.max(0) as usize)))
    }

    pub fn set_index_model(&self, model_id: &str, dimensions: usize) -> AppResult<()> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO index_meta (id, model_id, dimensions, indexed_at)
             VALUES (1, ?1, ?2, datetime('now'))
             ON CONFLICT(id) DO UPDATE SET
                model_id = excluded.model_id,
                dimensions = excluded.dimensions,
                indexed_at = excluded.indexed_at",
            params![model_id, dimensions as i64],
        )?;
        Ok(())
    }

    /// Tira el indice entero. Se llama cuando cambia el modelo de embeddings, porque los
    /// vectores viejos y los nuevos no son comparables aunque tengan la misma dimension.
    ///
    /// Se hace con DROP y no con DELETE: una tabla `vec0` lleva la dimension del vector
    /// en su declaracion, asi que vaciarla dejaria el ancho antiguo y las inserciones del
    /// modelo nuevo fallarian por dimension incompatible.
    pub fn clear_index(&self) -> AppResult<()> {
        let conn = self.lock()?;
        conn.execute_batch("DROP TABLE IF EXISTS chunk_vectors;")?;
        log::warn!("indice vectorial descartado");
        Ok(())
    }
}

/// Borrar vectores antes de que exista la tabla no es un error: significa que no habia
/// nada indexado.
fn ignore_missing_vector_table(err: rusqlite::Error) -> rusqlite::Result<usize> {
    match err {
        rusqlite::Error::SqliteFailure(_, Some(ref msg)) if msg.contains("no such table") => Ok(0),
        other => Err(other),
    }
}

fn read_document(conn: &Connection, id: i64) -> AppResult<Document> {
    conn.query_row(
        "SELECT d.id, d.project_id, d.title, d.kind, d.source_path, d.created_at,
                (SELECT count(*) FROM chunks c WHERE c.document_id = d.id)
         FROM documents d WHERE d.id = ?1",
        params![id],
        row_to_document,
    )
    .map_err(AppError::from)
}

fn row_to_document(row: &rusqlite::Row<'_>) -> rusqlite::Result<Document> {
    Ok(Document {
        id: row.get(0)?,
        project_id: row.get(1)?,
        title: row.get(2)?,
        kind: DocumentKind::parse(&row.get::<_, String>(3)?),
        source_path: row.get(4)?,
        created_at: row.get(5)?,
        chunk_count: row.get(6)?,
    })
}
