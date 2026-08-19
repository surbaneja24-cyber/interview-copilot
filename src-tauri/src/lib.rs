mod audio;
pub mod embedding;
mod error;
mod hardware;
mod llm;
mod platform;
mod rag;
mod secrets;
mod state;
mod storage;

use std::path::PathBuf;

use tauri::Manager;

use audio::{CaptureStatus, InputDevice};
use error::{AppError, AppResult};
use hardware::HardwareReport;
use llm::answering::{self, AnswerEvent};
use llm::prompt::AnswerStyle;
use llm::{LlmSettings, ProviderKind};
use rag::indexer::{IndexReport, Indexer};
use rag::retriever::{Retrieval, Retriever, DEFAULT_TOP_K};
use rag::{extract, vector_store};
use state::{AppState, ModelStatus};
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
    project_id: i64,
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
    let new = NewDocument {
        project_id,
        title,
        kind,
        source_path: Some(path),
        content,
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

/// Microfonos que ve el sistema. Se consulta cada vez y no se cachea: enchufar unos
/// cascos entre dos aperturas de Ajustes es lo normal, no la excepcion.
#[tauri::command]
fn audio_inputs() -> AppResult<Vec<InputDevice>> {
    audio::inputs()
}

/// Abre el microfono. Sin dispositivo, el que el sistema tenga por defecto.
#[tauri::command]
fn start_capture(
    state: tauri::State<'_, AppState>,
    device: Option<String>,
) -> AppResult<CaptureStatus> {
    state.start_capture(device)
}

#[tauri::command]
fn stop_capture(state: tauri::State<'_, AppState>) -> AppResult<()> {
    state.stop_capture()
}

/// Nivel y estado de la captura. La UI lo pide mientras dibuja la barra; por eso el nivel
/// no viaja por un `Channel` como la respuesta del LLM (ver `audio::capture`).
#[tauri::command]
fn capture_status(state: tauri::State<'_, AppState>) -> CaptureStatus {
    state.capture_status()
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
            index_pending,
            search_context,
            model_status,
            load_model,
            release_embedder,
            delete_all_data,
            audio_inputs,
            start_capture,
            stop_capture,
            capture_status,
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
