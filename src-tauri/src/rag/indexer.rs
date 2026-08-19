//! Pipeline de indexado: documento → trozos → embeddings → indice vectorial.
//!
//! Todo esto ocurre al preparar la entrevista, nunca durante. Es deliberado: es la parte
//! cara del sistema y no puede competir por CPU con la transcripcion en vivo.

use crate::embedding::EmbeddingProvider;
use crate::error::AppResult;
use crate::rag::{chunking, contact, vector_store};
use crate::storage::{Db, NewDocument};

/// Cuantos trozos se embeben de una vez. Lotes grandes van mas rapido pero reservan mas
/// memoria de golpe, y aqui la memoria es el recurso escaso.
const BATCH_SIZE: usize = 16;

pub struct Indexer<'a> {
    db: &'a Db,
    embedder: &'a dyn EmbeddingProvider,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexReport {
    pub documents: usize,
    pub chunks: usize,
    /// Verdadero si hubo que tirar el indice porque cambio el modelo de embeddings.
    pub reindexed_from_scratch: bool,
    /// Datos de contacto —correos, telefonos, perfiles— que no llegaron al indice (§31).
    /// Se enseña en la UI: es el unico dato con el que juzgar si la regla quita de mas o
    /// de menos.
    pub contact_data_removed: usize,
}

impl<'a> Indexer<'a> {
    pub fn new(db: &'a Db, embedder: &'a dyn EmbeddingProvider) -> Self {
        Self { db, embedder }
    }

    /// Comprueba que el indice existente se hizo con este mismo modelo. Si no, lo vacia:
    /// vectores de modelos distintos no son comparables aunque coincida la dimension, y
    /// mezclarlos degrada la recuperacion en silencio.
    fn ensure_index_matches_model(&self) -> AppResult<bool> {
        let expected = (self.embedder.id().to_owned(), self.embedder.dimensions());
        let current = self.db.index_model()?;

        let mismatch = match &current {
            Some(existing) => *existing != expected,
            None => false,
        };

        if mismatch {
            if let Some((old_id, _)) = current {
                log::warn!(
                    "el indice se construyo con {old_id} y ahora se usa {}: se reindexa",
                    expected.0
                );
            }
            self.db.clear_index()?;
        }

        self.db
            .with_conn(|conn| vector_store::create_index(conn, self.embedder.dimensions()))?;
        self.db.set_index_model(&expected.0, expected.1)?;

        Ok(mismatch)
    }

    /// Da de alta un documento y lo deja indexado.
    pub fn add_document(&self, new: &NewDocument) -> AppResult<IndexReport> {
        let reindexed = self.ensure_index_matches_model()?;

        let document = self.db.create_document(new)?;

        // El documento se guarda entero y se indexa sin los datos de contacto (§31). Lo
        // que se guarda es del usuario y no hay por que recortarselo; lo que se indexa es
        // lo que puede acabar en pantalla y en el prompt, y ahi un telefono solo gasta
        // uno de los cinco huecos.
        let cleaned = contact::strip(&new.content);
        let chunks = chunking::split(&cleaned.text);

        if chunks.is_empty() {
            return Ok(IndexReport {
                documents: 1,
                chunks: 0,
                reindexed_from_scratch: reindexed,
                contact_data_removed: cleaned.removed,
            });
        }

        let ids = self
            .db
            .replace_chunks(document.id, document.project_id, &chunks)?;

        let texts: Vec<String> = chunks.iter().map(|chunk| chunk.text.clone()).collect();
        self.embed_and_store(&ids, &texts)?;

        // Se registra cuantas lineas se dejaron fuera, nunca cuales: un log es un fichero
        // que se copia y se adjunta en un informe de errores (§31).
        log::info!(
            "documento \"{}\" indexado en {} trozos, {} datos de contacto fuera",
            document.title,
            chunks.len(),
            cleaned.removed
        );

        Ok(IndexReport {
            documents: 1,
            chunks: chunks.len(),
            reindexed_from_scratch: reindexed,
            contact_data_removed: cleaned.removed,
        })
    }

    /// Indexa lo que quedo pendiente de un proyecto: trozos sin vector, ya sea porque el
    /// modelo cambio o porque un indexado anterior se interrumpio.
    pub fn index_pending(&self, project_id: i64) -> AppResult<IndexReport> {
        let reindexed = self.ensure_index_matches_model()?;

        let pending = self.db.unindexed_chunks(project_id)?;
        if pending.is_empty() {
            return Ok(IndexReport {
                documents: 0,
                chunks: 0,
                reindexed_from_scratch: reindexed,
                // Los trozos pendientes ya pasaron por el filtro al darlos de alta.
                contact_data_removed: 0,
            });
        }

        let ids: Vec<i64> = pending.iter().map(|(id, _)| *id).collect();
        let texts: Vec<String> = pending.into_iter().map(|(_, text)| text).collect();
        self.embed_and_store(&ids, &texts)?;

        Ok(IndexReport {
            documents: 0,
            chunks: ids.len(),
            reindexed_from_scratch: reindexed,
            contact_data_removed: 0,
        })
    }

    fn embed_and_store(&self, ids: &[i64], texts: &[String]) -> AppResult<()> {
        for (id_batch, text_batch) in ids.chunks(BATCH_SIZE).zip(texts.chunks(BATCH_SIZE)) {
            let vectors = self.embedder.embed_documents(text_batch)?;

            self.db.with_conn(|conn| {
                for (id, vector) in id_batch.iter().zip(&vectors) {
                    vector_store::upsert(conn, *id, vector)?;
                }
                Ok(())
            })?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::AppError;
    use crate::storage::{DocumentKind, NewProject};

    /// Proveedor determinista: no descarga nada y produce vectores predecibles, para
    /// poder testear el pipeline sin depender de un modelo real.
    struct FakeEmbedder {
        id: &'static str,
        dimensions: usize,
    }

    impl EmbeddingProvider for FakeEmbedder {
        fn embed_documents(&self, texts: &[String]) -> AppResult<Vec<Vec<f32>>> {
            Ok(texts.iter().map(|text| self.vector_for(text)).collect())
        }

        fn embed_query(&self, text: &str) -> AppResult<Vec<f32>> {
            Ok(self.vector_for(text))
        }

        fn dimensions(&self) -> usize {
            self.dimensions
        }

        fn id(&self) -> &str {
            self.id
        }
    }

    impl FakeEmbedder {
        /// Vector estable derivado del texto: mismas palabras, mismo vector.
        fn vector_for(&self, text: &str) -> Vec<f32> {
            let mut vector = vec![0.0f32; self.dimensions];
            for (index, byte) in text.bytes().enumerate() {
                let slot = (byte as usize + index) % self.dimensions;
                vector[slot] += 1.0;
            }
            vector
        }
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        db: Db,
        project_id: i64,
    }

    fn fixture() -> Fixture {
        vector_store::register();
        let dir = tempfile::tempdir().expect("directorio temporal");
        let db = Db::open(&dir.path().join("test.db")).expect("abrir base");
        let project = db
            .create_project(&NewProject {
                name: "Prueba".into(),
                company: "ACME".into(),
                role: "Dev".into(),
            })
            .expect("crear proyecto");

        Fixture {
            _dir: dir,
            db,
            project_id: project.id,
        }
    }

    fn documento(project_id: i64, title: &str, content: &str) -> NewDocument {
        NewDocument {
            project_id,
            title: title.to_owned(),
            kind: DocumentKind::Cv,
            source_path: None,
            content: content.to_owned(),
        }
    }

    #[test]
    fn indexa_un_documento_y_deja_sus_trozos_con_vector() {
        let f = fixture();
        let embedder = FakeEmbedder {
            id: "fake",
            dimensions: 8,
        };
        let indexer = Indexer::new(&f.db, &embedder);

        let report = indexer
            .add_document(&documento(
                f.project_id,
                "CV",
                "Lideré la migración del monolito.\n\nDoy clases de matemáticas.",
            ))
            .expect("indexar");

        assert_eq!(report.documents, 1);
        assert!(report.chunks > 0);
        assert!(
            f.db.unindexed_chunks(f.project_id)
                .expect("pendientes")
                .is_empty(),
            "no deberia quedar ningun trozo sin vector"
        );
    }

    /// §31: la cabecera de un CV no llega al indice. No es cosmetico — un fragmento con
    /// el telefono ocupa uno de los cinco que se le mandan al modelo en cada pregunta.
    #[test]
    fn los_datos_de_contacto_no_llegan_al_indice() {
        let f = fixture();
        let embedder = FakeEmbedder {
            id: "fake",
            dimensions: 8,
        };

        let report = Indexer::new(&f.db, &embedder)
            .add_document(&documento(
                f.project_id,
                "CV",
                "SANTIAGO URBANEJA
                 Teléfono: 600 123 456
                 correo@ejemplo.com

                 EXPERIENCIA
                 Lideré la migración de un monolito PHP a microservicios en Node durante                  seis meses, coordinando a cuatro personas del equipo de plataforma.",
            ))
            .expect("indexar");

        assert_eq!(report.contact_data_removed, 2);

        let indexado = f
            .db
            .with_conn(|conn| {
                let mut stmt = conn.prepare("SELECT text FROM chunks WHERE project_id = ?1")?;
                let rows = stmt.query_map([f.project_id], |row| row.get::<_, String>(0))?;
                Ok(rows.collect::<Result<Vec<_>, _>>()?.join(" "))
            })
            .expect("leer trozos");
        assert!(!indexado.contains("600"), "el telefono llego al indice: {indexado}");
        assert!(!indexado.contains('@'), "el correo llego al indice: {indexado}");
        assert!(indexado.contains("Lideré la migración"));
    }

    #[test]
    fn cambiar_de_modelo_tira_el_indice_y_reindexa() {
        let f = fixture();

        let pequeno = FakeEmbedder {
            id: "modelo-a",
            dimensions: 8,
        };
        Indexer::new(&f.db, &pequeno)
            .add_document(&documento(f.project_id, "CV", "Experiencia en Rust."))
            .expect("primer indexado");

        assert_eq!(
            f.db.index_model().expect("modelo"),
            Some(("modelo-a".to_owned(), 8))
        );

        // Otro modelo, otra dimension.
        let grande = FakeEmbedder {
            id: "modelo-b",
            dimensions: 16,
        };
        let report = Indexer::new(&f.db, &grande)
            .index_pending(f.project_id)
            .expect("reindexar");

        assert!(
            report.reindexed_from_scratch,
            "deberia haber detectado el cambio"
        );
        assert!(report.chunks > 0, "deberia haber reindexado los trozos");
        assert_eq!(
            f.db.index_model().expect("modelo"),
            Some(("modelo-b".to_owned(), 16))
        );
    }

    #[test]
    fn reindexar_sin_cambios_no_repite_trabajo() {
        let f = fixture();
        let embedder = FakeEmbedder {
            id: "fake",
            dimensions: 8,
        };

        Indexer::new(&f.db, &embedder)
            .add_document(&documento(f.project_id, "CV", "Experiencia en Rust."))
            .expect("indexar");

        let report = Indexer::new(&f.db, &embedder)
            .index_pending(f.project_id)
            .expect("segundo pase");

        assert!(!report.reindexed_from_scratch);
        assert_eq!(report.chunks, 0, "no habia nada pendiente");
    }

    #[test]
    fn un_documento_sin_texto_se_rechaza() {
        let f = fixture();
        let embedder = FakeEmbedder {
            id: "fake",
            dimensions: 8,
        };

        let error = Indexer::new(&f.db, &embedder)
            .add_document(&documento(f.project_id, "Vacío", "   \n  "))
            .expect_err("deberia rechazarse");

        assert!(matches!(error, AppError::Invalid(_)));
    }

    /// Camino completo con el modelo de verdad: fichero en disco → extraccion → troceado
    /// → embeddings de 768 dimensiones → indice → busqueda. Los demas tests usan un
    /// proveedor falso, asi que este es el unico que probaria un fallo de dimension o de
    /// integracion con fastembed.
    ///
    /// `cargo test --lib -- --ignored --nocapture --test-threads=1 extremo_a_extremo`
    #[test]
    #[ignore = "descarga el modelo real"]
    fn extremo_a_extremo_con_el_modelo_real() {
        use crate::embedding::LocalEmbeddingProvider;
        use crate::rag::extract;
        use crate::rag::retriever::{Retriever, DEFAULT_TOP_K};

        let f = fixture();
        let cache = std::env::temp_dir().join("interview-copilot-models");
        let embedder = LocalEmbeddingProvider::new(&cache).expect("cargar el modelo real");

        // Un fichero de verdad en disco, no una cadena en memoria. Los parrafos tienen
        // longitud de CV real a proposito: con parrafos de dos lineas el documento entero
        // cabe en un solo trozo y la busqueda no tendria entre que elegir, con lo que
        // cualquier asercion pasaria sin comprobar nada.
        let cv = f._dir.path().join("cv.txt");
        std::fs::write(
            &cv,
            "EXPERIENCIA. Lideré la migración de un monolito PHP a una arquitectura de \
             microservicios en Node durante seis meses, coordinando a cuatro personas del \
             equipo de plataforma. Reduje el tiempo de despliegue de dos horas a once \
             minutos introduciendo integración continua y pruebas de humo automáticas. El \
             obstáculo mayor fue la base de datos compartida entre servicios, que resolvimos \
             con un patrón de strangler fig y doble escritura durante toda la transición, \
             sin ventanas de parada para el cliente.\n\n\
             DOCENCIA. Di clases particulares de matemáticas a estudiantes de bachillerato \
             durante tres años, preparando sobre todo la selectividad. Aprendí a detectar \
             en qué punto exacto se había roto la comprensión de cada alumno y a reconstruir \
             el razonamiento desde ahí en vez de repetir el temario. Llevé a la vez hasta \
             seis alumnos con niveles muy distintos, adaptando el material a cada uno y \
             manteniendo un registro semanal de su progreso.\n\n\
             ATENCIÓN AL CLIENTE. Trabajé dos veranos en una cafetería del centro llevando \
             la caja y atendiendo la barra en las horas punta del mediodía. Gestionaba pedidos \
             simultáneos, cobros y reclamaciones con el local lleno, y me tocó resolver más \
             de una queja subida de tono sin perder la compostura ni hacer esperar al resto \
             de la cola. También cuadraba la caja al cierre y hacía el pedido de proveedores.",
        )
        .expect("escribir el CV de prueba");

        let content = extract::from_file(&cv).expect("extraer texto");
        let report = Indexer::new(&f.db, &embedder)
            .add_document(&documento(f.project_id, "cv.txt", &content))
            .expect("indexar");

        // Sin varios fragmentos no hay nada que elegir y la asercion final pasaria sola.
        assert!(
            report.chunks >= 3,
            "solo salieron {} fragmentos: la busqueda no tendria entre que elegir y este \
             test pasaria sin comprobar nada",
            report.chunks
        );
        println!("indexados {} fragmentos", report.chunks);

        let retrieval = Retriever::new(&f.db, &embedder)
            .search(
                f.project_id,
                "¿Tienes experiencia enseñando a otras personas?",
                DEFAULT_TOP_K,
            )
            .expect("buscar");

        assert!(!retrieval.chunks.is_empty(), "la busqueda no devolvio nada");
        for chunk in &retrieval.chunks {
            println!("  {:.4}  {}", chunk.similarity, chunk.chunk.text);
        }

        let mejor = &retrieval.chunks[0].chunk.text;
        assert!(
            mejor.contains("clases particulares"),
            "el primer resultado deberia ser el de docencia, y fue: {mejor}"
        );
        assert!(
            !mejor.contains("strangler fig"),
            "el fragmento de docencia y el de la migracion salieron pegados: el troceado \
             no esta separando experiencias distintas"
        );
    }

    #[test]
    fn borrar_un_documento_se_lleva_sus_vectores() {
        let f = fixture();
        let embedder = FakeEmbedder {
            id: "fake",
            dimensions: 8,
        };
        let indexer = Indexer::new(&f.db, &embedder);

        indexer
            .add_document(&documento(f.project_id, "CV", "Experiencia en Rust."))
            .expect("indexar");

        let documents = f.db.list_documents(f.project_id).expect("listar");
        let document_id = documents[0].id;
        f.db.delete_document(document_id).expect("borrar");

        let remaining: i64 =
            f.db.with_conn(|conn| {
                conn.query_row("SELECT count(*) FROM chunk_vectors", [], |row| row.get(0))
                    .map_err(AppError::from)
            })
            .expect("contar vectores");

        assert_eq!(remaining, 0, "quedaron vectores huerfanos tras borrar");
    }
}
