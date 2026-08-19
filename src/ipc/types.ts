/**
 * Espejo en TypeScript de los tipos que serializa el backend en Rust.
 * Si cambias un struct con `#[derive(Serialize)]`, cambia también este archivo:
 * son dos lenguajes distintos y nada los sincroniza automáticamente todavía.
 */

export type ExecutionProfile = 'LOCAL' | 'HYBRID' | 'CLOUD';

export interface Recommendation {
  readonly profile: ExecutionProfile;
  readonly sttModel: string;
  /** `null` si no cabe ningún modelo local con la memoria disponible. */
  readonly localLlm: string | null;
  /** Si es `false`, el modelo local sirve para practicar pero no para una entrevista. */
  readonly realtimeLocalLlm: boolean;
  readonly reasons: readonly string[];
}

export interface GpuInfo {
  readonly name: string;
  /** Lo que declara el adaptador. En una integrada es un recorte de la RAM del sistema. */
  readonly dedicatedVramMb: number;
  readonly sharedMemoryMb: number;
  readonly discrete: boolean;
}

export interface HardwareReport {
  readonly os: string;
  readonly cpuBrand: string;
  readonly logicalCores: number;
  readonly totalRamMb: number;
  readonly availableRamMb: number;
  readonly gpus: readonly GpuInfo[];
  /** `null` si ninguna gráfica aporta memoria propia utilizable. */
  readonly dedicatedVramMb: number | null;
  readonly recommendation: Recommendation;
}

export interface Project {
  readonly id: number;
  readonly name: string;
  readonly company: string;
  readonly role: string;
  readonly createdAt: string;
  readonly updatedAt: string;
}

export interface NewProject {
  readonly name: string;
  readonly company: string;
  readonly role: string;
}

export const DOCUMENT_KINDS = [
  'cv',
  'job_offer',
  'company',
  'notes',
  'prepared_answers',
  'other',
] as const;

export type DocumentKind = (typeof DOCUMENT_KINDS)[number];

export interface DocumentInfo {
  readonly id: number;
  /** `null` cuando el documento es del candidato y vale para todas las entrevistas. */
  readonly projectId: number | null;
  readonly title: string;
  readonly kind: DocumentKind;
  /** Para una respuesta preparada, el tipo de pregunta que contesta. */
  readonly tag: string | null;
  readonly sourcePath: string | null;
  readonly createdAt: string;
  /** Cero significa que entró pero no se indexó. */
  readonly chunkCount: number;
}

export interface IndexReport {
  readonly documents: number;
  readonly chunks: number;
  /** El modelo de embeddings cambió y hubo que rehacer el índice entero. */
  readonly reindexedFromScratch: boolean;
  /**
   * Correos, teléfonos y perfiles que se quedaron fuera del índice (§31). Se enseña:
   * es el único dato con el que juzgar si el filtro quita de más o de menos.
   */
  readonly contactDataRemoved: number;
}

export interface RetrievedChunk {
  readonly id: number;
  readonly documentId: number;
  readonly documentTitle: string;
  readonly kind: DocumentKind;
  readonly ordinal: number;
  readonly text: string;
  readonly similarity: number;
}

export interface Retrieval {
  readonly chunks: readonly RetrievedChunk[];
  /**
   * Cuánto se despegó el mejor resultado de la media del resto. Es diagnóstico, NO
   * significa "hay experiencia relevante": se midió y ninguna señal de similitud sirve
   * para eso (ver `docs/ARCHITECTURE.md` §5).
   */
  readonly standout: number;
  readonly weakSignal: boolean;
}

export interface ModelStatus {
  /** El modelo está en memoria y listo para usar. */
  readonly loaded: boolean;
  /** Bytes que lleva escritos en la carpeta de modelos. Es lo único observable del
   *  progreso de descarga: fastembed no lo expone. */
  readonly bytesOnDisk: number;
  readonly expectedBytes: number;
  readonly modelId: string;
}

// ---------------------------------------------------------------------------
// LLM (fase 3)
// ---------------------------------------------------------------------------

/** `mock` solo existe en compilaciones de desarrollo; el backend dice cuáles hay. */
export type ProviderKind = 'local' | 'open_ai' | 'mock';

export interface LlmSettings {
  readonly kind: ProviderKind;
  readonly baseUrl: string;
  readonly model: string;
  readonly temperature: number;
  readonly maxTokens: number;
  readonly jsonMode: boolean;
}

export type AnswerStyle = 'behavioral' | 'technical' | 'general';

export interface FragmentSummary {
  readonly number: number;
  readonly documentTitle: string;
  readonly preview: string;
}

export interface VerifiedCitation {
  readonly fragment: number;
  readonly chunkId: number;
  readonly documentTitle: string;
  readonly quote: string;
}

export interface DroppedCitation {
  readonly fragment: number;
  readonly quote: string;
  readonly reason: 'emptyQuote' | 'quoteNotFound';
}

/** Por qué se descartó la respuesta. Espejo de `llm::verify::Unsupported`. */
export type UnsupportedDetail =
  | { readonly reason: 'noContext' }
  | { readonly reason: 'modelFoundNothing' }
  | { readonly reason: 'noCitations' }
  | { readonly reason: 'malformedCitations'; readonly seen: number }
  | { readonly reason: 'inventedFragment'; readonly fragment: number }
  | { readonly reason: 'noLiteralSupport'; readonly dropped: readonly DroppedCitation[] };

/**
 * Lo que llega por el canal mientras se genera una respuesta.
 * `delta` solo se emite con las citas ya verificadas: el backend nunca manda texto
 * que luego haya que retirar (§6).
 */
export type AnswerEvent =
  | { readonly event: 'retrieved'; readonly fragments: readonly FragmentSummary[]; readonly sentTo: string }
  | { readonly event: 'delta'; readonly text: string }
  | {
      readonly event: 'completed';
      readonly answer: string;
      readonly keyPoints: readonly string[];
      readonly followUps: readonly string[];
      readonly citations: readonly VerifiedCitation[];
      readonly dropped: readonly DroppedCitation[];
      readonly elapsedMs: number;
    }
  | {
      readonly event: 'unsupported';
      readonly explanation: string;
      readonly detail: UnsupportedDetail;
      readonly structure: readonly string[];
    }
  | { readonly event: 'failed'; readonly message: string };

// ---------------------------------------------------------------------------
// Audio (fase 4)
// ---------------------------------------------------------------------------

export interface InputDevice {
  /**
   * Identificador estable entre reinicios y reconexiones, que da cpal. Es lo que se
   * manda al backend para abrir: el nombre no distingue dos tarjetas iguales.
   */
  readonly id: string;
  readonly name: string;
  readonly isDefault: boolean;
  readonly channels: number;
  readonly sampleRate: number;
}

/**
 * De dónde se captura. `system` es el loopback de WASAPI: lo que suena por los altavoces
 * o los auriculares, o sea la voz del entrevistador en la videollamada.
 */
export type Source = 'mic' | 'system';

/** Decibelios a fondo de escala: 0 es el máximo y el suelo es −100, nunca −∞. */
export interface AudioLevel {
  readonly rmsDbfs: number;
  readonly peakDbfs: number;
}

/** Estado del detector de voz. */
export interface VadState {
  readonly turn: 'silent' | 'speaking';
  /** Probabilidad de la última ventana, de 0 a 1. Se enseña para ver con qué margen se decide. */
  readonly probability: number;
  /**
   * La más alta desde que arrancó la captura. Es el dato con el que se sabrá si el umbral
   * está bien puesto: sin él, un VAD que casi no dispara y uno que dispara de sobra se ven
   * igual en pantalla.
   */
  readonly maxProbability: number;
  /** Duración del último turno cerrado, en milisegundos de voz. */
  readonly lastTurnMs: number | null;
  readonly turns: number;
  /** Muestras que el VAD no llegó a ver porque la cola se llenó. */
  readonly dropped: number;
  /** La muestra más alta que ha visto el detector: distingue "no hay voz" de "no llega audio". */
  readonly peakIn: number;
}

export interface CaptureStatus {
  readonly source: Source;
  readonly capturing: boolean;
  readonly device: string | null;
  readonly sampleRate: number;
  readonly channels: number;
  readonly level: AudioLevel;
  /** Muestras recibidas. Cero con la captura abierta significa que no llega nada. */
  readonly frames: number;
  /** Fallo posterior al arranque: el dispositivo se fue a mitad. */
  readonly error: string | null;
  /** `null` si no hay modelo de VAD descargado, que no es lo mismo que "no hay voz". */
  readonly vad: VadState | null;
}

export interface CaptureSnapshot {
  readonly mic: CaptureStatus;
  readonly system: CaptureStatus;
  /** El indicador de §11, resuelto en el backend: MIC / SYSTEM AUDIO / BOTH / OFF. */
  readonly indicator: string;
}

// ---------------------------------------------------------------------------
// Transcripción (fase 4)
// ---------------------------------------------------------------------------

export interface SttModelStatus {
  /** El identificador que usa el detector de hardware al recomendar. */
  readonly id: string;
  readonly file: string;
  readonly sha256: string;
  readonly bytes: number;
  readonly downloaded: boolean;
  /** El que el detector de hardware recomienda para este equipo. */
  readonly recommended: boolean;
}

export interface TranscriptEntry {
  readonly source: Source;
  readonly text: string;
  readonly audioMs: number;
  /** Lo que tardó whisper. Es el número que decide si el modo LOCAL es usable (§10). */
  readonly tookMs: number;
}

export interface TranscriptState {
  readonly entries: readonly TranscriptEntry[];
  /** Turnos esperando turno de whisper. Si crece, el equipo no da abasto. */
  readonly pending: number;
  readonly model: string;
  readonly loaded: boolean;
  readonly error: string | null;
}

// ---------------------------------------------------------------------------
// Entrenamiento (§5)
// ---------------------------------------------------------------------------

export type QuestionKind =
  | 'behavioral'
  | 'motivation'
  | 'experience'
  | 'situational'
  | 'selfAssessment'
  | 'logistics';

export interface TrainingStatus {
  readonly id: string;
  readonly kind: QuestionKind;
  readonly text: string;
  /** Qué tiene que llevar dentro una buena respuesta. */
  readonly hint: string;
  /** El documento con la respuesta, si ya se contestó. */
  readonly answer: number | null;
}
