mod audio;
mod download;
pub mod embedding;
mod error;
mod hardware;
mod llm;
mod platform;
mod rag;
mod secrets;
mod state;
mod storage;
mod stt;
mod training;

use std::path::PathBuf;

use tauri::Manager;

use audio::{CaptureStatus, InputDevice, Source};
use error::{AppError, AppResult};
use hardware::HardwareReport;
use llm::answering::{self, AnswerEvent};
use llm::prompt::AnswerStyle;
use llm::{LlmSettings, ProviderKind};
use rag::indexer::{IndexReport, Indexer};
use rag::retriever::{Retrieval, Retriever, DEFAULT_TOP_K};
use rag::{extract, vector_store};
use state::{AppState, CaptureSnapshot, ModelStatus};
use stt::{SttModel, TranscriptState};
use training::TrainingQuestion;
use storage::{Db, Document, DocumentKind, NewDocument, NewProject, Project};

const DB_FILE: &str = "interview-copilot.db";
const MODELS_DIR: &str = "models";

#[tauri::command]
fn hardware_report() -> HardwareReport {
    hardware::detect()
}

#[tauri::command]
fn create_project(state: tauri::State<'_, AppState>, project: NewProject) -> AppResult<Project> {
    state.db.create_project(&project)
}

#[tauri::command]
fn list_projects(state: tauri::State<'_, AppState>) -> AppResult<Vec<Project>> {
    state.db.list_projects()
}

#[tauri::command]
fn delete_project(state: tauri::State<'_, AppState>, id: i64) -> AppResult<()> {
    state.db.delete_project(id)
}

#[tauri::command]
fn list_documents(state: tauri::State<'_, AppState>, project_id: i64) -> AppResult<Vec<Document>> {
    state.db.list_documents(project_id)
}

#[tauri::command]
fn delete_document(state: tauri::State<'_, AppState>, id: i64) -> AppResult<()> {
    state.db.delete_document(id)
}

/// Carga un fichero del disco, extrae su texto y lo indexa.
///
/// Es `async` a proposito: un comando sincrono de Tauri corre en el hilo principal y
/// congelaria la ventana durante los segundos que tardan la carga del modelo y el
/// indexado. Como `async`, corre en el pool de tokio y la UI sigue respondiendo.
#[tauri::command]
async fn import_document(
    state: tauri::State<'_, AppState>,
    project_id: Option<i64>,
    path: String,
    kind: DocumentKind,
) -> AppResult<IndexReport> {
    let source = PathBuf::from(&path);

    let title = source
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("documento")
        .to_owned();

    let content = extract::from_file(&source)?;
    log::info!("extraidos {} caracteres de {title}", content.len());

    let embedder = state.embedder()?;
    // Sin proyecto, el documento es del candidato y sirve para todas las entrevistas: es
    // lo que hace que el CV se cargue una vez y no en cada oferta.
    let new = NewDocument {
        project_id,
        title,
        kind,
        tag: None,
        source_path: Some(path),
        content,
    };

    Indexer::new(&state.db, embedder.as_ref()).add_document(&new)
}

/// El material del candidato: lo que no cuelga de ninguna entrevista.
#[tauri::command]
fn candidate_documents(
    state: tauri::State<'_, AppState>,
    kind: Option<DocumentKind>,
) -> AppResult<Vec<Document>> {
    state.db.list_candidate_documents(kind)
}

/// El banco de entrenamiento, con lo que ya esta contestado.
///
/// Una pregunta se da por contestada si existe un documento del candidato con esa misma
/// pregunta por titulo. No se guarda el identificador en la base a proposito: asi una
/// respuesta escrita a mano a una pregunta que no esta en el banco cuenta igual.
#[tauri::command]
fn training_questions(state: tauri::State<'_, AppState>) -> AppResult<Vec<TrainingStatus>> {
    let answered = state
        .db
        .list_candidate_documents(Some(DocumentKind::PreparedAnswers))?;

    Ok(training::QUESTIONS
        .iter()
        .map(|question| TrainingStatus {
            question: *question,
            answer: answered
                .iter()
                .find(|document| document.title == question.text)
                .map(|document| document.id),
        })
        .collect())
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct TrainingStatus {
    #[serde(flatten)]
    question: TrainingQuestion,
    /// El documento con la respuesta, si ya se contesto.
    answer: Option<i64>,
}

/// Guarda una respuesta preparada y la indexa (§5).
///
/// Se guarda **la pregunta junto a la respuesta**, no la respuesta sola. Es lo que hace que
/// durante la entrevista una pregunta parecida recupere esta respuesta: el parecido esta
/// entre preguntas, no entre la pregunta y el texto de una respuesta que habla de otra cosa.
///
/// Y no cuelga de ningun proyecto a proposito: lo que el usuario contesta sobre si mismo
/// vale para esta entrevista y para las siguientes, que es justo el punto del entrenamiento.
#[tauri::command]
async fn save_prepared_answer(
    state: tauri::State<'_, AppState>,
    question: String,
    answer: String,
    tag: Option<String>,
) -> AppResult<IndexReport> {
    let question = question.trim().to_owned();
    let answer = answer.trim().to_owned();

    if question.is_empty() || answer.is_empty() {
        return Err(AppError::Invalid(
            "hacen falta la pregunta y la respuesta".into(),
        ));
    }

    let embedder = state.embedder()?;
    let new = NewDocument {
        project_id: None,
        title: question.clone(),
        kind: DocumentKind::PreparedAnswers,
        tag,
        source_path: None,
        content: format!("Pregunta: {question}

Respuesta: {answer}"),
    };

    Indexer::new(&state.db, embedder.as_ref()).add_document(&new)
}

/// Indexa lo que quedara pendiente, por ejemplo tras cambiar de modelo.
#[tauri::command]
async fn index_pending(
    state: tauri::State<'_, AppState>,
    project_id: i64,
) -> AppResult<IndexReport> {
    let embedder = state.embedder()?;
    Indexer::new(&state.db, embedder.as_ref()).index_pending(project_id)
}

/// Busqueda manual sobre el indice. Es la herramienta para comprobar con los ojos si la
/// recuperacion acierta, antes de que haya ningun LLM de por medio.
#[tauri::command]
async fn search_context(
    state: tauri::State<'_, AppState>,
    project_id: i64,
    question: String,
) -> AppResult<Retrieval> {
    if question.trim().is_empty() {
        return Err(AppError::Invalid("Escribe una pregunta".into()));
    }

    let embedder = state.embedder()?;
    Retriever::new(&state.db, embedder.as_ref()).search(project_id, &question, DEFAULT_TOP_K)
}

#[tauri::command]
fn model_status(state: tauri::State<'_, AppState>) -> ModelStatus {
    state.model_status()
}

/// Fuerza la carga del modelo. Existe para que la UI pueda ofrecer un boton explicito en
/// vez de que la primera descarga de 1 GB ocurra por sorpresa al cargar un documento.
#[tauri::command]
async fn load_model(state: tauri::State<'_, AppState>) -> AppResult<()> {
    state.embedder()?;
    Ok(())
}

/// Libera el modelo de embeddings. Antes de una entrevista conviene: es ~1 GB que la
/// transcripcion y el LLM van a necesitar mas.
#[tauri::command]
fn release_embedder(state: tauri::State<'_, AppState>) -> AppResult<()> {
    state.release_embedder()
}

/// §15 del spec. Es destructivo e irreversible: la confirmacion la pide la UI.
///
/// Se lleva tambien las claves de API, que no viven en la base sino en el almacen de
/// credenciales del sistema. "Borrar todos mis datos" y dejar la clave guardada seria
/// mentir en el boton.
#[tauri::command]
fn delete_all_data(state: tauri::State<'_, AppState>) -> AppResult<()> {
    state.db.delete_all_data()?;

    for kind in ProviderKind::WITH_CREDENTIALS {
        secrets::clear(kind.credential_id())?;
    }
    state.invalidate_llm();

    Ok(())
}

// ---------------------------------------------------------------------------
// Audio (§11)
// ---------------------------------------------------------------------------

/// Dispositivos que ve el sistema para una fuente. Para el audio del sistema son las
/// salidas, que es como funciona el loopback.
///
/// Se consulta cada vez y no se cachea: enchufar unos cascos entre dos aperturas de
/// Ajustes es lo normal, no la excepcion.
#[tauri::command]
fn audio_devices(source: Source) -> AppResult<Vec<InputDevice>> {
    audio::devices(source)
}

/// Abre una fuente. Sin dispositivo, el que el sistema tenga por defecto.
#[tauri::command]
fn start_capture(
    state: tauri::State<'_, AppState>,
    source: Source,
    device: Option<String>,
) -> AppResult<CaptureStatus> {
    state.start_capture(source, device)
}

#[tauri::command]
fn stop_capture(state: tauri::State<'_, AppState>, source: Source) -> AppResult<()> {
    state.stop_capture(source)
}

/// Si el modelo del VAD esta descargado. La UI lo usa para ofrecer la descarga en vez de
/// fallar al arrancar la captura.
#[tauri::command]
fn vad_model_present(state: tauri::State<'_, AppState>) -> bool {
    state.vad_model().is_some()
}

/// Descarga el modelo del VAD (2,2 MB). Es `async` porque toca la red: un comando sincrono
/// congelaria la ventana mientras dura.
#[tauri::command]
async fn download_vad_model(state: tauri::State<'_, AppState>) -> AppResult<()> {
    audio::vad::ensure_model(state.models_dir()).await?;
    Ok(())
}

/// Nivel y estado de las dos fuentes. La UI lo pide mientras dibuja las barras; por eso el
/// nivel no viaja por un `Channel` como la respuesta del LLM (ver `audio::capture`).
#[tauri::command]
fn capture_status(state: tauri::State<'_, AppState>) -> CaptureSnapshot {
    state.capture_status()
}

/// Modelos de transcripcion, con cual esta descargado y cual recomienda el hardware.
#[tauri::command]
fn stt_models(state: tauri::State<'_, AppState>) -> Vec<SttModelStatus> {
    let recomendado = hardware::detect().recommendation.stt_model;

    stt::MODELS
        .iter()
        .map(|model| SttModelStatus {
            model: *model,
            downloaded: model.is_downloaded(state.models_dir()),
            recommended: model.id == recomendado,
        })
        .collect()
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SttModelStatus {
    #[serde(flatten)]
    model: SttModel,
    downloaded: bool,
    /// El que recomienda el detector de hardware para este equipo (§4).
    recommended: bool,
}

/// Descarga un modelo de transcripcion. Son entre 75 y 490 MB, asi que no se descarga
/// nada sin que el usuario lo pida (§2).
#[tauri::command]
async fn download_stt_model(state: tauri::State<'_, AppState>, id: String) -> AppResult<()> {
    let model = stt::model_by_id(&id)
        .ok_or_else(|| AppError::Invalid(format!("no existe el modelo {id}")))?;

    model.ensure(state.models_dir()).await?;
    Ok(())
}

/// Lo transcrito en esta sesion. `None` mientras no haya arrancado ninguna captura con
/// modelo de transcripcion.
#[tauri::command]
fn transcript(state: tauri::State<'_, AppState>) -> Option<TranscriptState> {
    state.transcript()
}

/// Suelta el modelo de whisper (~200 MB).
#[tauri::command]
fn release_transcriber(state: tauri::State<'_, AppState>) -> AppResult<()> {
    state.release_transcriber()
}

// ---------------------------------------------------------------------------
// LLM (§18, §19, §31)
// ---------------------------------------------------------------------------

#[tauri::command]
fn llm_settings(state: tauri::State<'_, AppState>) -> AppResult<LlmSettings> {
    state.llm_settings()
}

#[tauri::command]
fn save_llm_settings(state: tauri::State<'_, AppState>, settings: LlmSettings) -> AppResult<()> {
    state.save_llm_settings(&settings)
}

/// Proveedores que puede elegir el usuario. Lo decide el backend porque el simulador solo
/// existe en compilaciones de desarrollo.
#[tauri::command]
fn llm_providers() -> Vec<ProviderKind> {
    ProviderKind::SELECTABLE.to_vec()
}

/// Modelos que declara el servidor configurado. Es tambien la comprobacion de que hay
/// alguien al otro lado antes de una entrevista.
#[tauri::command]
async fn llm_models(state: tauri::State<'_, AppState>) -> AppResult<Vec<String>> {
    let provider = state.llm_provider()?;
    provider.models().await
}

/// Guarda la clave en el almacen de credenciales del sistema.
///
/// **No existe el comando simetrico para leerla.** §31 pide que las claves no se muestren
/// en la interfaz, y la forma de garantizarlo es que el frontend no tenga por donde
/// pedirla: solo puede preguntar si hay una puesta.
#[tauri::command]
fn set_api_key(state: tauri::State<'_, AppState>, provider: ProviderKind, key: String) -> AppResult<()> {
    secrets::store(provider.credential_id(), &key)?;
    state.invalidate_llm();
    Ok(())
}

#[tauri::command]
fn api_key_present(provider: ProviderKind) -> bool {
    secrets::has(provider.credential_id())
}

#[tauri::command]
fn clear_api_key(state: tauri::State<'_, AppState>, provider: ProviderKind) -> AppResult<()> {
    secrets::clear(provider.credential_id())?;
    state.invalidate_llm();
    Ok(())
}

/// El comando de la Fase 3: pregunta escrita a mano, respuesta con sus fuentes.
///
/// Los resultados salen por un `Channel` y no como valor de retorno porque la respuesta
/// se ensena mientras se escribe. Lo que llega por ese canal ya esta verificado: el texto
/// sin respaldo no sale de `answering`.
#[tauri::command]
async fn ask(
    state: tauri::State<'_, AppState>,
    project_id: i64,
    question: String,
    style: AnswerStyle,
    on_event: tauri::ipc::Channel<AnswerEvent>,
) -> AppResult<()> {
    if question.trim().is_empty() {
        return Err(AppError::Invalid("Escribe una pregunta".into()));
    }

    let settings = state.llm_settings()?;
    let provider = state.llm_provider()?;
    let embedder = state.embedder()?;

    let mut emit = |event: AnswerEvent| {
        if let Err(err) = on_event.send(event) {
            log::warn!("no se pudo enviar el evento a la UI: {err}");
        }
    };

    answering::Answering {
        db: &state.db,
        embedder: embedder.as_ref(),
        provider: provider.as_ref(),
        settings: &settings,
    }
    .answer(project_id, &question, style, &mut emit)
    .await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Tiene que ocurrir antes de abrir ninguna conexion: sqlite-vec se registra como
    // extension automatica y solo la ven las conexiones creadas despues.
    vector_store::register();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        // Los destinos se declaran a mano en vez de fiarse del defecto del
                        // plugin: el fichero de log dejo de escribirse en algun momento y
                        // eso costo una depuracion a ciegas. Un log que a veces esta es
                        // peor que no tenerlo, porque se cuenta con el.
                        .clear_targets()
                        .target(tauri_plugin_log::Target::new(
                            tauri_plugin_log::TargetKind::Stdout,
                        ))
                        .target(tauri_plugin_log::Target::new(
                            tauri_plugin_log::TargetKind::LogDir { file_name: None },
                        ))
                        .build(),
                )?;
            }

            // Los datos del usuario viven en su perfil, nunca junto al ejecutable.
            let data_dir = app.path().app_data_dir()?;
            let db = Db::open(&data_dir.join(DB_FILE))?;
            app.manage(AppState::new(db, data_dir.join(MODELS_DIR)));

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            hardware_report,
            create_project,
            list_projects,
            delete_project,
            list_documents,
            delete_document,
            import_document,
            candidate_documents,
            training_questions,
            save_prepared_answer,
            index_pending,
            search_context,
            model_status,
            load_model,
            release_embedder,
            delete_all_data,
            audio_devices,
            start_capture,
            stop_capture,
            capture_status,
            vad_model_present,
            download_vad_model,
            stt_models,
            download_stt_model,
            transcript,
            release_transcriber,
            llm_settings,
            save_llm_settings,
            llm_providers,
            llm_models,
            set_api_key,
            api_key_present,
            clear_api_key,
            ask
        ])
        .run(tauri::generate_context!())
        .expect("error arrancando la aplicacion");
}
