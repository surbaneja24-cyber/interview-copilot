//! Pipeline de indexado: documento → trozos → embeddings → indice vectorial.
//!
//! Todo esto ocurre al preparar la entrevista, nunca durante. Es deliberado: es la parte
//! cara del sistema y no puede competir por CPU con la transcripcion en vivo.

use crate::embedding::EmbeddingProvider;
use crate::error::AppResult;
use crate::rag::{chunking, contact, vector_store};
use crate::storage::{Db, DocumentKind, NewDocument};

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
        let mut chunks = chunking::split(&cleaned.text);

        // Una respuesta preparada lleva su pregunta en **cada** trozo.
        //
        // Medido el 2026-08-19: sin esto, el troceador separaba "Pregunta: …" de
        // "Respuesta: …" —son dos parrafos y juntos pasan del limite para fusionarlos— y el
        // mejor resultado de la busqueda acababa siendo el fragmento que solo tiene la
        // pregunta. Recuperaba de maravilla (0,93) y no servia para nada: al modelo le
        // llegaba la pregunta que ya sabia y no la respuesta que hacia falta.
        if new.kind == DocumentKind::PreparedAnswers {
            for chunk in &mut chunks {
                chunk.text = format!("{}
{}", document.title, chunk.text);
            }
        }

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
    use crate::storage::NewProject;

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
            project_id: Some(project_id),
            title: title.to_owned(),
            kind: DocumentKind::Cv,
            tag: None,
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

    /// El material del candidato no cuelga de ningun proyecto y aun asi se recupera desde
    /// uno. Es la propiedad sobre la que se apoya el entrenamiento: se contesta una vez y
    /// vale para todas las entrevistas.
    ///
    /// El dia que la busqueda filtre por proyecto —hoy no lo hace— este test tiene que
    /// seguir pasando, y por eso esta escrito.
    #[test]
    fn una_respuesta_del_candidato_se_recupera_desde_cualquier_proyecto() {
        let f = fixture();
        let embedder = FakeEmbedder {
            id: "fake",
            dimensions: 8,
        };

        let respuesta = NewDocument {
            project_id: None,
            title: "¿Cuál es tu mayor defecto?".into(),
            kind: DocumentKind::PreparedAnswers,
            tag: Some("selfAssessment".into()),
            source_path: None,
            content: "Pregunta: ¿Cuál es tu mayor defecto?\nRespuesta: Me cuesta \n                      delegar, y lo llevo compensando desde que me tocó repartir el trabajo \n                      de un turno entero entre cuatro personas."
                .into(),
        };

        Indexer::new(&f.db, &embedder)
            .add_document(&respuesta)
            .expect("guardar la respuesta");

        // Un proyecto nuevo, creado despues, sin ningun documento propio.
        let otra_entrevista = f
            .db
            .create_project(&NewProject {
                name: "Otra empresa".into(),
                company: "Otra".into(),
                role: "Mozo".into(),
            })
            .expect("crear proyecto");

        let encontrado = crate::rag::retriever::Retriever::new(&f.db, &embedder)
            .search(
                otra_entrevista.id,
                "¿Cuál es tu mayor defecto?",
                5,
                crate::rag::retriever::Material::All,
            )
            .expect("buscar");

        assert!(
            encontrado
                .chunks
                .iter()
                .any(|chunk| chunk.chunk.text.contains("delegar")),
            "la respuesta entrenada no llego a una entrevista distinta"
        );
    }

    /// **La premisa del entrenamiento, medida con el modelo real:** ante una pregunta de
    /// entrevista, la respuesta que el candidato preparo gana al CV.
    ///
    /// Si esto no se cumpliera, entrenar no serviria de nada: el modelo seguiria recibiendo
    /// las mismas lineas telegraficas del curriculum y teniendo que rellenar los huecos.
    ///
    /// `cargo test --lib -- --ignored --nocapture la_respuesta_entrenada_gana`
    #[test]
    #[ignore = "descarga el modelo real"]
    fn la_respuesta_entrenada_gana_al_cv() {
        use crate::embedding::LocalEmbeddingProvider;
        use crate::rag::retriever::{Material, Retriever, DEFAULT_TOP_K};

        let f = fixture();
        let cache = std::env::temp_dir().join("interview-copilot-models");
        let embedder = LocalEmbeddingProvider::new(&cache).expect("cargar el modelo real");
        let indexer = Indexer::new(&f.db, &embedder);

        indexer
            .add_document(&documento(
                f.project_id,
                "cv.txt",
                "EXPERIENCIA. Mozo de almacén y gestión logística en Supply Rodamientos. Carga,                  descarga y reubicación de mercancía. Preparación diaria de pedidos, picking y                  packing, para venta directa y expedición de envíos. Organización del inventario                  físico manteniendo el orden y la seguridad en la zona de trabajo.

                 COMPETENCIAS. Capacidad de trabajo físico pesado. Organización metódica.                  Trabajo en equipo. Resolución rápida de incidencias. Carnet de carretillero.",
            ))
            .expect("indexar el CV");

        let pregunta = "Cuéntame una vez que tuviste un conflicto con un compañero";
        let entrenada = NewDocument {
            project_id: None,
            title: pregunta.into(),
            kind: DocumentKind::PreparedAnswers,
            tag: Some("behavioral".into()),
            source_path: None,
            content: format!(
                "Pregunta: {pregunta}\nRespuesta: Un compañero del turno de tarde \n                 dejaba la zona de picking sin recoger y yo me la encontraba cada mañana. \n                 En vez de decírselo al encargado, se lo dije a él directamente y acordamos \n                 repasar juntos los últimos quince minutos del turno. Dejó de pasar en una \n                 semana y acabamos llevándonos bien."
            ),
        };
        indexer
            .add_document(&entrenada)
            .expect("indexar la respuesta entrenada");

        let recuperado = Retriever::new(&f.db, &embedder)
            .search(f.project_id, pregunta, DEFAULT_TOP_K, Material::All)
            .expect("buscar");

        for chunk in &recuperado.chunks {
            println!("  {:.4}  {}", chunk.similarity, &chunk.chunk.text[..80.min(chunk.chunk.text.len())]);
        }

        let mejor = &recuperado.chunks[0].chunk;
        assert!(
            mejor.text.contains("turno de tarde"),
            "gano el CV en vez de la respuesta entrenada: {}",
            mejor.text
        );
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
        use crate::rag::retriever::{Material, Retriever, DEFAULT_TOP_K};

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
                Material::All,
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

    /// El CV real de Santiago, tal y como esta indexado en su base (8 fragmentos, 1.923
    /// caracteres). Se copia aqui en vez de leerlo del disco para que la medicion sea
    /// repetible: el CV de la base cambia cuando el lo cambie, y entonces los numeros de
    /// abajo dejarian de ser comparables sin que nadie se entere.
    const CV_REAL: &str = "Santiago Urbaneja\n\nProfesional de logística y almacén\n\nIgualada, España\n\n\
         Perfil Profesional\n\n\
         Profesional de logística y almacén proactivo y organizado, con casi tres años de \
         experiencia en la gestión integral de inventarios, preparación de pedidos y control \
         de stock. Poseo carnet de carretillero en vigor y destreza en el manejo de mercancías. \
         Acostumbrado a entornos de trabajo dinámicos, combinando el esfuerzo físico con la \
         gestión administrativa (Excel/ Word).\n\n\
         Experiencia Laboral\n\n\
         Mozo de Almacén y Gestión Logística, Supply Rodamientos\n\n\
         •Carga, descarga y reubicación de mercancía, asegurando el correcto manejo de los \
         materiales dentro de las instalaciones.\n\n\
         •Preparación diaria de pedidos (picking y packing) para venta directa y expedición de \
         envíos, cumpliendo estrictamente con los tiempos de entrega.\n\n\
         10/2021 – 09/2024\n\n\
         •Organización eficiente del inventario físico, manteniendo el orden, la limpieza y la \
         seguridad en la zona de trabajo.\n\n\
         •Atención directa al cliente y resolución de incidencias, combinando el trabajo \
         operativo con soporte en ventas presenciales y online.\n\n\
         Educación y Formación\n\nBachillerato\n\nIngeniería en Mantenimiento (2 semestres)\n\n\
         Habilidades y Competencias\n\nOperativa Logística\n\n•Carga y descarga\n\n\
         •Preparación de pedidos (picking/ packing)\n\n•Control de stock\n\n\
         •Recepción de mercancías\n\nGestión Administrativa\n\n\
         Nivel avanzado de Excel y Word para el seguimiento de inventarios y cuadre de albaranes\n\n\
         Maquinaria y Equipos\n\nCarnet de Carretillero\n\nCompetencias Transversales\n\n\
         •Capacidad de trabajo físico pesado\n\n•Organización metódica\n\n•Trabajo en equipo\n\n\
         •Resolución rápida de incidencias\n\nIdiomas\n\nEspañol Nativo\n\nInglés Nivel B1\n\n\
         Catalán Básico\n\nCertificaciones y Carnets\n\n\
         Carnet de Carretillero. Curso técnico: Manejo de pisos y acabados de Resina Epoxy. \
         Cursos certificados en atención al cliente: Bartender y Barista.";

    /// Una oferta del puesto que ese CV persigue. Es material **de la empresa**, no del
    /// candidato, y esa es toda la gracia del test.
    const OFERTA: &str = "Oferta de empleo: Operario/a de Almacén con carretilla — Igualada\n\n\
         Buscamos incorporar a nuestro centro logístico una persona responsable y metódica \
         para la gestión integral del almacén. Te encargarás de la recepción de mercancía, \
         la ubicación del stock y la preparación de pedidos con los plazos de entrega \
         acordados con el cliente.\n\n\
         Funciones principales\n\n\
         •Carga y descarga de camiones con carretilla frontal y transpaleta eléctrica.\n\n\
         •Preparación de pedidos mediante picking y packing, garantizando el cumplimiento de \
         los tiempos de expedición.\n\n\
         •Control de inventario y cuadre de albaranes en el sistema, con soporte de Excel.\n\n\
         •Resolución de incidencias con transportistas y atención al cliente interno.\n\n\
         Requisitos\n\n\
         •Carnet de carretillero en vigor y experiencia mínima de dos años en almacén.\n\n\
         •Capacidad para el trabajo físico y para mantener el orden y la seguridad en planta.\n\n\
         •Persona organizada, resolutiva y acostumbrada a trabajar en equipo bajo presión.\n\n\
         •Se valorará nivel de inglés y disponibilidad para turnos rotativos.\n\n\
         Ofrecemos contrato indefinido, salario según convenio y plan de formación continua.";

    /// **De dónde salen los cinco fragmentos que ve el modelo.**
    ///
    /// El retriever ordena solo por similitud, y el origen de cada fragmento
    /// (`DocumentKind`) no pinta nada en esa decision. El comentario de `DocumentKind` ya
    /// dice que sirve "para pesar la recuperacion mas adelante"; esto mide si ese "mas
    /// adelante" hace falta, antes de poner ninguna constante.
    ///
    /// Lo que se busca no es una nota media, son dos numeros concretos:
    ///
    /// 1. **Cuantos fragmentos de la oferta entran en el top 5.** La oferta dice lo que la
    ///    empresa pide, no lo que el candidato ha hecho. Si se cuela, el modelo puede
    ///    componer una respuesta con ella y ademas citarla literalmente: la barrera de §5
    ///    la daria por buena, porque esa frase si esta en los documentos. Es el camino
    ///    exacto de inventar experiencia que §6 prohibe.
    /// 2. **Con que margen.** Si la oferta entra por los pelos, un peso pequeno la saca; si
    ///    entra arrasando, el problema no se arregla pesando.
    ///
    /// Corpus: el CV real y una oferta del puesto que persigue. Las preguntas son el banco
    /// entero de entrenamiento, sin elegir.
    ///
    /// `cargo test --lib -- --ignored --nocapture de_donde_salen_los_cinco`
    #[test]
    #[ignore = "descarga el modelo real"]
    fn de_donde_salen_los_cinco_fragmentos_que_ve_el_modelo() {
        use crate::embedding::LocalEmbeddingProvider;
        use crate::rag::retriever::{Material, Retriever, DEFAULT_TOP_K};

        let f = fixture();
        let cache = std::env::temp_dir().join("interview-copilot-models");
        let embedder = LocalEmbeddingProvider::new(&cache).expect("cargar el modelo real");
        let indexer = Indexer::new(&f.db, &embedder);

        indexer
            .add_document(&documento(f.project_id, "CV de Santiago", CV_REAL))
            .expect("indexar el CV");
        indexer
            .add_document(&NewDocument {
                project_id: Some(f.project_id),
                title: "Oferta de almacén".into(),
                kind: DocumentKind::JobOffer,
                tag: None,
                source_path: None,
                content: OFERTA.into(),
            })
            .expect("indexar la oferta");

        let retriever = Retriever::new(&f.db, &embedder);

        let sin_filtro = reparto(&retriever, f.project_id, Material::All, "SIN FILTRO");
        let con_filtro = reparto(
            &retriever,
            f.project_id,
            Material::CandidateOnly,
            "CON FILTRO (Material::CandidateOnly)",
        );

        let total = crate::training::QUESTIONS.len();

        // Lo que hay que demostrar es lo de siempre en este proyecto: que el arreglo hace
        // lo que dice, no que exista. Sin filtro, la oferta entraba en casi todas.
        assert!(
            sin_filtro.preguntas_contaminadas > total / 2,
            "sin filtro la oferta solo entraba en {} de {total}: si el corpus ha cambiado \
             tanto, los numeros de ARCHITECTURE.md §5.2 hay que volver a sacarlos",
            sin_filtro.preguntas_contaminadas
        );

        // Y con el filtro, ni una. No es un "muchas menos": el material de la empresa no
        // tiene grados cuando la pregunta va sobre lo que hizo el candidato.
        assert_eq!(
            con_filtro.fragmentos, 0,
            "el filtro dejo pasar {} fragmentos de la oferta",
            con_filtro.fragmentos
        );

        // Y los sitios que libera los tiene que ocupar material del candidato, no dejarlos
        // vacios. Un top 5 que se queda en tres es media respuesta.
        assert_eq!(
            con_filtro.sitios_llenos,
            total * DEFAULT_TOP_K,
            "el filtro dejo huecos: {} fragmentos de los {} que caben",
            con_filtro.sitios_llenos,
            total * DEFAULT_TOP_K
        );
    }

    /// Lo que sale de recorrer el banco entero con un material dado.
    struct Reparto {
        preguntas_contaminadas: usize,
        fragmentos: usize,
        primeras: usize,
        sitios_llenos: usize,
    }

    /// Recorre las veinte preguntas y cuenta cuanto material de la empresa se cuela.
    fn reparto(
        retriever: &crate::rag::retriever::Retriever<'_>,
        project_id: i64,
        material: crate::rag::retriever::Material,
        titulo: &str,
    ) -> Reparto {
        use crate::rag::retriever::DEFAULT_TOP_K;

        let mut out = Reparto {
            preguntas_contaminadas: 0,
            fragmentos: 0,
            primeras: 0,
            sitios_llenos: 0,
        };

        println!("\n=== {titulo} ===");
        println!("{:<62} {:>7} {:>7}", "pregunta", "empresa", "1º");

        for question in crate::training::QUESTIONS {
            let recuperado = retriever
                .search(project_id, question.text, DEFAULT_TOP_K, material)
                .expect("buscar");

            let de_la_empresa = recuperado
                .chunks
                .iter()
                .filter(|chunk| chunk.chunk.kind.speaks_for_the_employer())
                .count();
            let primero_es_empresa = recuperado
                .chunks
                .first()
                .is_some_and(|chunk| chunk.chunk.kind.speaks_for_the_employer());

            out.fragmentos += de_la_empresa;
            out.sitios_llenos += recuperado.chunks.len();
            if de_la_empresa > 0 {
                out.preguntas_contaminadas += 1;
            }
            if primero_es_empresa {
                out.primeras += 1;
            }

            println!(
                "{:<62} {:>7} {:>7}",
                &question.text[..62.min(question.text.len())],
                format!("{de_la_empresa}/{}", recuperado.chunks.len()),
                if primero_es_empresa { "OFERTA" } else { "cv" }
            );
        }

        let total = crate::training::QUESTIONS.len();
        println!(
            "RESUMEN {titulo}: {} de {total} preguntas con material de la empresa en el top 5. \
             {} fragmentos de oferta sobre {} sitios. La oferta es la primera en {}.",
            out.preguntas_contaminadas,
            out.fragmentos,
            total * DEFAULT_TOP_K,
            out.primeras
        );

        out
    }
}
