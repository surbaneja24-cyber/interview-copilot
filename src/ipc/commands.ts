import { Channel, invoke } from '@tauri-apps/api/core';
import type {
  AnswerEvent,
  AnswerStyle,
  CaptureStatus,
  DocumentInfo,
  DocumentKind,
  HardwareReport,
  IndexReport,
  InputDevice,
  LlmSettings,
  ModelStatus,
  NewProject,
  Project,
  ProviderKind,
  Retrieval,
} from '@/ipc/types';

/**
 * Único punto por el que el frontend habla con Rust. Ninguna vista debe llamar a
 * `invoke` directamente: así los nombres de comando y sus tipos viven en un solo sitio.
 */

export function hardwareReport(): Promise<HardwareReport> {
  return invoke<HardwareReport>('hardware_report');
}

export function listProjects(): Promise<readonly Project[]> {
  return invoke<Project[]>('list_projects');
}

export function createProject(project: NewProject): Promise<Project> {
  return invoke<Project>('create_project', { project });
}

export async function deleteProject(id: number): Promise<void> {
  await invoke('delete_project', { id });
}

export function listDocuments(projectId: number): Promise<readonly DocumentInfo[]> {
  return invoke<DocumentInfo[]>('list_documents', { projectId });
}

export async function deleteDocument(id: number): Promise<void> {
  await invoke('delete_document', { id });
}

/** Lee el fichero, extrae su texto y lo indexa. Puede tardar: carga el modelo. */
export function importDocument(
  projectId: number,
  path: string,
  kind: DocumentKind,
): Promise<IndexReport> {
  return invoke<IndexReport>('import_document', { projectId, path, kind });
}

export function searchContext(projectId: number, question: string): Promise<Retrieval> {
  return invoke<Retrieval>('search_context', { projectId, question });
}

export function modelStatus(): Promise<ModelStatus> {
  return invoke<ModelStatus>('model_status');
}

/** Descarga (si hace falta) y carga el modelo. Puede tardar minutos la primera vez. */
export async function loadModel(): Promise<void> {
  await invoke('load_model');
}

/** Libera el modelo de embeddings (~1 GB) cuando ya no hace falta indexar. */
export async function releaseEmbedder(): Promise<void> {
  await invoke('release_embedder');
}

/** Destructivo e irreversible (§15). Confirmar antes de llamar. */
export async function deleteAllData(): Promise<void> {
  await invoke('delete_all_data');
}

// ---------------------------------------------------------------------------
// LLM (fase 3)
// ---------------------------------------------------------------------------

export function llmSettings(): Promise<LlmSettings> {
  return invoke<LlmSettings>('llm_settings');
}

export async function saveLlmSettings(settings: LlmSettings): Promise<void> {
  await invoke('save_llm_settings', { settings });
}

/** Lo decide el backend: el simulador solo existe en compilaciones de desarrollo. */
export function llmProviders(): Promise<readonly ProviderKind[]> {
  return invoke<ProviderKind[]>('llm_providers');
}

/** Modelos que declara el servidor. Falla si no hay nadie escuchando, que es el punto. */
export function llmModels(): Promise<readonly string[]> {
  return invoke<string[]>('llm_models');
}

/**
 * Guarda la clave en el almacén de credenciales del sistema.
 * No existe la operación inversa a propósito: §31 prohíbe enseñar claves en la interfaz,
 * y la forma de garantizarlo es que el frontend no tenga por dónde pedirlas.
 */
export async function setApiKey(provider: ProviderKind, key: string): Promise<void> {
  await invoke('set_api_key', { provider, key });
}

export function apiKeyPresent(provider: ProviderKind): Promise<boolean> {
  return invoke<boolean>('api_key_present', { provider });
}

export async function clearApiKey(provider: ProviderKind): Promise<void> {
  await invoke('clear_api_key', { provider });
}

/**
 * Pregunta y respuesta con fuentes. Los resultados llegan por `onEvent` según se
 * generan; la promesa se resuelve cuando termina todo.
 */
export async function ask(
  projectId: number,
  question: string,
  style: AnswerStyle,
  onEvent: (event: AnswerEvent) => void,
): Promise<void> {
  const channel = new Channel<AnswerEvent>();
  channel.onmessage = onEvent;
  await invoke('ask', { projectId, question, style, onEvent: channel });
}

// ---------------------------------------------------------------------------
// Audio (fase 4)
// ---------------------------------------------------------------------------

/** Se consulta cada vez: enchufar unos cascos entre dos visitas a Ajustes es lo normal. */
export function audioInputs(): Promise<readonly InputDevice[]> {
  return invoke<InputDevice[]>('audio_inputs');
}

/** `null` abre el micrófono que el sistema tenga por defecto. */
export function startCapture(device: string | null): Promise<CaptureStatus> {
  return invoke<CaptureStatus>('start_capture', { device });
}

export async function stopCapture(): Promise<void> {
  await invoke('stop_capture');
}

/**
 * Nivel y estado de la captura. Se pregunta al ritmo al que se dibuja la barra; el nivel
 * no viaja por un `Channel` porque serían cien mensajes por segundo para un dibujo.
 */
export function captureStatus(): Promise<CaptureStatus> {
  return invoke<CaptureStatus>('capture_status');
}
