//! Estado compartido de la aplicacion.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::audio::vad::{self, Turn, VadModel};
use crate::interview::session::{Session, Snapshot};
use crate::audio::{CaptureStatus, Recorder, Source};
use crate::stt::{self, TranscriptState, Transcriber};
use crate::embedding::LocalEmbeddingProvider;
use crate::error::{AppError, AppResult};
use crate::llm::settings::SETTINGS_KEY;
use crate::llm::{HttpProvider, LlmProvider, LlmSettings, ProviderKind};
use crate::secrets;
use crate::storage::Db;

/// Lo que la pantalla ve de la entrevista.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InterviewView {
    #[serde(flatten)]
    pub state: crate::interview::machine::State,
    /// Turnos descartados por no parecer preguntas. A la vista porque lo que se tira hay que
    /// poder verlo: si esto sube rapido, el filtro se esta comiendo preguntas.
    pub skipped: usize,
    /// La pregunta que hay que preparar, si se ha pedido recogerla.
    ///
    /// **No se puede llamar `question`**: el estado aplanado ya trae un campo con ese nombre
    /// y uno pisaria al otro en silencio. Lo caza `vista_tests`.
    pub pending_question: Option<String>,
}

pub struct AppState {
    pub db: Db,
    models_dir: PathBuf,
    /// El modelo de embeddings ocupa ~1 GB y tarda segundos en cargar, asi que no se
    /// carga al arrancar: solo la primera vez que hace falta indexar o buscar. En una
    /// maquina de 5,7 GB eso es la diferencia entre poder abrir la app para mirar los
    /// proyectos y tener que esperar a que se cargue un modelo que quiza no se use.
    ///
    /// Una sola instancia por proceso, ademas, porque dos cargas simultaneas del mismo
    /// modelo se pisan en el directorio de cache y una de las dos falla.
    embedder: Mutex<Option<Arc<LocalEmbeddingProvider>>>,
    /// El provider de LLM se cachea para conservar el pool de conexiones entre preguntas:
    /// rehacer el handshake TLS en cada pregunta son cientos de milisegundos regalados,
    /// y la latencia es el punto mas critico del producto (§10).
    ///
    /// Se invalida al cambiar los ajustes o la clave, no se actualiza en caliente: es la
    /// unica forma de que un cambio en Ajustes no deje viva una conexion a un extremo que
    /// el usuario acaba de dejar de usar.
    llm: Mutex<Option<Arc<dyn LlmProvider>>>,
    /// Las capturas en marcha, una por fuente. Dos flujos sobre el **mismo** dispositivo
    /// se pelean y ninguno entrega nada util; microfono y salida son dispositivos
    /// distintos y se capturan a la vez, que es justo lo que hace falta en una entrevista.
    mic: Mutex<Option<Recorder>>,
    system: Mutex<Option<Recorder>>,
    /// El hilo que transcribe los turnos cerrados. Se crea con la primera captura que
    /// tenga modelo, y el modelo de whisper no se carga hasta el primer turno: son ~200 MB
    /// y esta maquina los necesita para otras cosas mientras nadie hable.
    transcriber: Mutex<Option<Transcriber>>,
    /// La entrevista en marcha.
    ///
    /// Vive en el estado y no en la pantalla porque **la entrevista no puede depender de que
    /// una ventana siga abierta**: el usuario cambia a Ajustes a mirar el medidor y vuelve, y
    /// lo que se dijo mientras tanto sigue siendo parte de la misma entrevista.
    interview: Mutex<Session>,
    /// El modelo del VAD, cargado una vez y compartido por las dos capturas.
    ///
    /// Son 2,3 MB en disco y 250 ms de construir la sesion de ONNX Runtime, medidos en
    /// §4.6. Cargarlo en cada `Recorder::start` —que es lo que se hacia— ponia esos 250 ms
    /// por delante de abrir el dispositivo, una vez por pregunta en el modo diapositiva, y
    /// lo que se hablase mientras tanto no se descartaba: no llegaba a existir.
    ///
    /// Se carga bajo demanda y no al arrancar, como el resto: sin el modelo descargado la
    /// app tiene que abrir igual. Y si la carga falla no se guarda nada, con lo que el
    /// fallo sigue apareciendo al pulsar "Escuchar" y no en un hilo que muere en silencio.
    vad: Mutex<Option<VadModel>>,
}

impl AppState {
    pub fn new(db: Db, models_dir: PathBuf) -> Self {
        Self {
            db,
            models_dir,
            embedder: Mutex::new(None),
            llm: Mutex::new(None),
            mic: Mutex::new(None),
            system: Mutex::new(None),
            transcriber: Mutex::new(None),
            interview: Mutex::new(Session::new()),
            vad: Mutex::new(None),
        }
    }

    pub fn llm_settings(&self) -> AppResult<LlmSettings> {
        Ok(self.db.load_settings(SETTINGS_KEY)?.unwrap_or_default())
    }

    pub fn save_llm_settings(&self, settings: &LlmSettings) -> AppResult<()> {
        self.db.save_settings(SETTINGS_KEY, settings)?;
        self.invalidate_llm();
        Ok(())
    }

    /// Devuelve el provider configurado, construyendolo si hace falta.
    pub fn llm_provider(&self) -> AppResult<Arc<dyn LlmProvider>> {
        let mut slot = self
            .llm
            .lock()
            .map_err(|err| AppError::Poisoned(err.to_string()))?;

        if let Some(existing) = slot.as_ref() {
            return Ok(Arc::clone(existing));
        }

        let settings = self.llm_settings()?;
        let provider = build_provider(&settings)?;
        *slot = Some(Arc::clone(&provider));

        Ok(provider)
    }

    pub fn invalidate_llm(&self) {
        if let Ok(mut slot) = self.llm.lock() {
            slot.take();
        }
    }

    fn slot(&self, source: Source) -> &Mutex<Option<Recorder>> {
        match source {
            Source::Mic => &self.mic,
            Source::System => &self.system,
        }
    }

    /// Arranca la captura de una fuente, parando la que hubiera de esa misma fuente.
    ///
    /// El orden importa y es al reves de lo que parece: primero se suelta la anterior
    /// —soltarla es lo que cierra el dispositivo— y solo despues se abre la nueva. Al
    /// reves, cambiar de microfono fallaria porque el anterior seguiria cogido.
    pub fn start_capture(&self, source: Source, device: Option<String>) -> AppResult<CaptureStatus> {
        let mut slot = self
            .slot(source)
            .lock()
            .map_err(|err| AppError::Poisoned(err.to_string()))?;

        slot.take();
        // Si el modelo esta descargado, la captura arranca con deteccion de voz; si no,
        // arranca sin ella. Obligar a descargar 2 MB antes de poder ver el medidor seria
        // poner una puerta donde no hace falta. Lo mismo con la transcripcion: sin modelo
        // de whisper hay VAD y no hay texto.
        let model = self.vad_model()?;
        let turns = self.turn_sink()?;
        let recorder = Recorder::start(source, device, model, turns)?;
        let status = recorder.status();
        *slot = Some(recorder);

        Ok(status)
    }

    /// Entrar en la entrevista.
    pub fn interview_enter(&self) -> AppResult<InterviewView> {
        let entries = self.transcript().map_or(0, |state| state.entries.len());
        {
            let mut session = self.interview_lock()?;
            session.enter(entries);
        }
        self.look(false)
    }

    pub fn interview_leave(&self) -> AppResult<InterviewView> {
        self.interview_lock()?.leave();
        self.look(false)
    }

    /// Anota que la sugerencia llego, o que no va a llegar.
    pub fn interview_suggestion(&self, ok: bool) -> AppResult<InterviewView> {
        self.interview_lock()?.suggestion(ok);
        self.look(false)
    }

    /// Mira el mundo, adelanta la maquina y devuelve el estado.
    ///
    /// `recoger` se lleva la pregunta pendiente: quien la pide se compromete a prepararla, y
    /// dejarla ahi haria que se preparase otra vez en el sondeo siguiente. Consultar el
    /// estado para dibujar no se la lleva.
    pub fn interview_poll(&self, recoger: bool) -> AppResult<InterviewView> {
        let transcript = self.transcript();
        let entries = transcript.map(|state| state.entries).unwrap_or_default();
        let hablando = |source: Source| {
            self.status_of(source)
                .vad
                .is_some_and(|vad| vad.turn == Turn::Speaking)
        };

        let snapshot = Snapshot {
            entries: &entries,
            mic_speaking: hablando(Source::Mic),
            system_speaking: hablando(Source::System),
        };

        let mut session = self.interview_lock()?;
        session.observe(&snapshot);
        drop(session);

        self.look(recoger)
    }

    fn look(&self, recoger: bool) -> AppResult<InterviewView> {
        let mut session = self.interview_lock()?;
        Ok(InterviewView {
            state: session.state().clone(),
            skipped: session.skipped(),
            pending_question: if recoger { session.take_pending() } else { None },
        })
    }

    fn interview_lock(&self) -> AppResult<std::sync::MutexGuard<'_, Session>> {
        self.interview
            .lock()
            .map_err(|err| AppError::Poisoned(err.to_string()))
    }

    pub fn models_dir(&self) -> &std::path::Path {
        &self.models_dir
    }

    /// Devuelve por donde mandar los turnos a transcribir, creando el hilo si hace falta.
    ///
    /// Sin modelo de whisper descargado devuelve `None`, y entonces la captura funciona
    /// igual pero sin texto: eso es mejor que negarse a capturar.
    fn turn_sink(&self) -> AppResult<Option<crate::stt::transcriber::TurnSink>> {
        let Some((path, id)) = self.stt_model() else {
            return Ok(None);
        };

        let mut slot = self
            .transcriber
            .lock()
            .map_err(|err| AppError::Poisoned(err.to_string()))?;

        if slot.is_none() {
            *slot = Some(Transcriber::start(path, id)?);
        }

        Ok(slot.as_ref().and_then(Transcriber::sender))
    }

    /// El modelo de whisper descargado que toque usar: el que recomienda el hardware si
    /// esta, y si no cualquiera que haya, de mayor a menor.
    fn stt_model(&self) -> Option<(PathBuf, &'static str)> {
        let recomendado = crate::hardware::detect().recommendation.stt_model;

        let preferido = stt::model_by_id(recomendado)
            .filter(|model| model.is_downloaded(&self.models_dir));

        preferido
            .or_else(|| {
                stt::MODELS
                    .iter()
                    .rev()
                    .find(|model| model.is_downloaded(&self.models_dir))
            })
            .map(|model| (model.path(&self.models_dir), model.id))
    }

    /// Lo transcrito hasta ahora. Vacio y sin modelo si nadie ha capturado todavia.
    pub fn transcript(&self) -> Option<TranscriptState> {
        self.transcriber
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().map(Transcriber::state))
    }

    /// Suelta el modelo de whisper. Son ~200 MB que el LLM necesita mas que nadie cuando
    /// la entrevista ha terminado.
    pub fn release_transcriber(&self) -> AppResult<()> {
        let mut slot = self
            .transcriber
            .lock()
            .map_err(|err| AppError::Poisoned(err.to_string()))?;

        if slot.take().is_some() {
            log::info!("modelo de transcripcion liberado");
        }

        Ok(())
    }

    /// Ruta del modelo del VAD si esta descargado. No lo carga: solo mira si esta.
    pub fn vad_model_path(&self) -> Option<PathBuf> {
        let path = vad::model_path(&self.models_dir);
        path.is_file().then_some(path)
    }

    /// El modelo del VAD cargado, cargandolo la primera vez que hace falta.
    ///
    /// `None` sin modelo descargado, y entonces la captura funciona igual pero sin
    /// deteccion de voz: obligar a descargar 2 MB antes de poder ver el medidor seria
    /// poner una puerta donde no hace falta.
    pub fn vad_model(&self) -> AppResult<Option<VadModel>> {
        let Some(path) = self.vad_model_path() else {
            return Ok(None);
        };

        let mut slot = self
            .vad
            .lock()
            .map_err(|err| AppError::Poisoned(err.to_string()))?;

        if slot.is_none() {
            let empezo = std::time::Instant::now();
            *slot = Some(VadModel::load(&path)?);
            log::info!("modelo del VAD cargado en {} ms", empezo.elapsed().as_millis());
        }

        Ok(slot.clone())
    }

    pub fn stop_capture(&self, source: Source) -> AppResult<()> {
        let mut slot = self
            .slot(source)
            .lock()
            .map_err(|err| AppError::Poisoned(err.to_string()))?;

        if slot.take().is_some() {
            log::info!("captura de audio detenida ({source:?})");
        }

        Ok(())
    }

    /// Estado y nivel de las dos fuentes. Lo llama la UI varias veces por segundo mientras
    /// haya una barra en pantalla, asi que no puede hacer nada caro: leer unos atomicos.
    pub fn capture_status(&self) -> CaptureSnapshot {
        let mic = self.status_of(Source::Mic);
        let system = self.status_of(Source::System);

        CaptureSnapshot {
            indicator: indicator(mic.capturing, system.capturing),
            mic,
            system,
        }
    }

    fn status_of(&self, source: Source) -> CaptureStatus {
        self.slot(source)
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().map(Recorder::status))
            .unwrap_or_else(|| CaptureStatus::idle(source))
    }

    /// Devuelve el proveedor de embeddings, cargandolo si es la primera vez.
    pub fn embedder(&self) -> AppResult<Arc<LocalEmbeddingProvider>> {
        let mut slot = self
            .embedder
            .lock()
            .map_err(|err| AppError::Poisoned(err.to_string()))?;

        if let Some(existing) = slot.as_ref() {
            return Ok(Arc::clone(existing));
        }

        log::info!("cargando el modelo de embeddings (primera vez, puede tardar)");
        let provider = Arc::new(LocalEmbeddingProvider::new(&self.models_dir)?);
        *slot = Some(Arc::clone(&provider));

        Ok(provider)
    }

    /// Libera el modelo. La entrevista en vivo necesita esa memoria mas que el indexado,
    /// que ya termino.
    pub fn release_embedder(&self) -> AppResult<()> {
        let mut slot = self
            .embedder
            .lock()
            .map_err(|err| AppError::Poisoned(err.to_string()))?;

        if slot.take().is_some() {
            log::info!("modelo de embeddings liberado");
        }

        Ok(())
    }

    pub fn embedder_loaded(&self) -> bool {
        self.embedder
            .lock()
            .map(|slot| slot.is_some())
            .unwrap_or(false)
    }

    /// Estado del modelo para la UI.
    ///
    /// `fastembed` no expone el progreso de su descarga, asi que se mide lo unico
    /// observable desde fuera: cuantos bytes lleva escritos en la carpeta de modelos. Es
    /// aproximado, pero es progreso real y no una animacion que finge que algo avanza.
    pub fn model_status(&self) -> ModelStatus {
        ModelStatus {
            loaded: self.embedder_loaded(),
            bytes_on_disk: directory_size(&self.models_dir),
            expected_bytes: crate::embedding::DEFAULT_MODEL.approx_bytes,
            model_id: crate::embedding::DEFAULT_MODEL.id,
        }
    }
}

/// Construye el provider que digan los ajustes, buscando la clave en el almacen del
/// sistema si el proveedor la necesita.
///
/// La clave se lee aqui y se pasa hacia dentro; no queda guardada en el estado de la
/// aplicacion. Cuantos menos sitios la toquen, mejor (§31).
fn build_provider(settings: &LlmSettings) -> AppResult<Arc<dyn LlmProvider>> {
    #[cfg(debug_assertions)]
    if settings.kind == ProviderKind::Mock {
        log::warn!("proveedor simulado activo: las respuestas no vienen de ninguna IA");
        return Ok(Arc::new(crate::llm::mock::MockProvider));
    }

    let api_key = if settings.kind.needs_api_key() {
        secrets::read(settings.kind.credential_id())?
    } else {
        None
    };

    Ok(Arc::new(HttpProvider::from_settings(settings, api_key)?))
}

/// Las dos fuentes de audio a la vez, con el indicador de §11 ya resuelto.
///
/// Quien decide si esto es MIC, SYSTEM AUDIO o BOTH es el backend, y no la UI, para que no
/// existan dos versiones de la misma regla.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureSnapshot {
    pub mic: CaptureStatus,
    pub system: CaptureStatus,
    pub indicator: &'static str,
}

fn indicator(mic: bool, system: bool) -> &'static str {
    match (mic, system) {
        (true, true) => "BOTH",
        (true, false) => "MIC",
        (false, true) => "SYSTEM AUDIO",
        (false, false) => "OFF",
    }
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelStatus {
    pub loaded: bool,
    pub bytes_on_disk: u64,
    pub expected_bytes: u64,
    pub model_id: &'static str,
}

fn directory_size(path: &std::path::Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };

    entries
        .filter_map(Result::ok)
        .map(|entry| match entry.file_type() {
            Ok(kind) if kind.is_dir() => directory_size(&entry.path()),
            Ok(_) => entry.metadata().map(|meta| meta.len()).unwrap_or(0),
            Err(_) => 0,
        })
        .sum()
}

#[cfg(test)]
mod vista_tests {
    use super::*;
    use crate::interview::machine::State;

    /// La forma exacta del JSON que ve la pantalla.
    ///
    /// `InterviewView` aplana el estado, y el estado **tambien** lleva un campo con la
    /// pregunta. Dos campos con el mismo nombre en un `flatten` no dan error de compilacion:
    /// uno pisa al otro en silencio, y el que pierde es el que la pantalla necesitaba.
    #[test]
    fn la_vista_no_pisa_la_pregunta_del_estado() {
        let vista = InterviewView {
            state: State::Preparing { question: "¿Por qué quieres trabajar aquí?".into() },
            skipped: 2,
            pending_question: Some("¿Por qué quieres trabajar aquí?".into()),
        };

        let json = serde_json::to_value(&vista).expect("serializar");
        println!("{json}");

        assert_eq!(json["state"], "preparing");
        assert_eq!(json["question"], "¿Por qué quieres trabajar aquí?");
        assert_eq!(json["pendingQuestion"], "¿Por qué quieres trabajar aquí?");
        assert_eq!(json["skipped"], 2);
    }
}
