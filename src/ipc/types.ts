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
  readonly projectId: number;
  readonly title: string;
  readonly kind: DocumentKind;
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
