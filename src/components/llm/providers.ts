import type { ProviderKind } from '@/ipc/types';

/**
 * Qué es cada proveedor y con qué valores arranca.
 *
 * Las URLs y los modelos son un espejo de `ProviderKind::default_base_url` y
 * `default_model` en Rust. Está duplicado a sabiendas: la alternativa era un viaje más al
 * backend solo para rellenar un campo al cambiar de proveedor, y el valor se persiste
 * igualmente, así que el backend sigue mandando.
 */

export const PROVIDER_LABELS: Record<ProviderKind, string> = {
  local: 'Local (Ollama o llama-server)',
  open_ai: 'OpenAI',
  mock: 'Simulador — sin IA, solo desarrollo',
};

/** Qué sale del equipo con cada uno. Es lo que §15 obliga a decir sin rodeos. */
export const PROVIDER_NOTES: Record<ProviderKind, string> = {
  local:
    'Nada sale de tu equipo. Necesita Ollama o llama-server arrancado antes de preguntar.',
  open_ai:
    'La pregunta y los fragmentos recuperados se envían a api.openai.com. El audio, el resto de tus documentos y las transcripciones no.',
  mock:
    'Devuelve una respuesta fabricada a partir de tus propios fragmentos, sin consultar ninguna IA. Sirve para comprobar que el flujo funciona; no existe en la versión distribuible.',
};

export function defaultBaseUrl(kind: ProviderKind): string {
  switch (kind) {
    case 'local':
      return 'http://localhost:11434/v1';
    case 'open_ai':
      return 'https://api.openai.com/v1';
    case 'mock':
      return '(ninguno)';
  }
}

export function defaultModel(kind: ProviderKind): string {
  switch (kind) {
    case 'local':
      return 'qwen2.5:3b-instruct';
    case 'open_ai':
      return 'gpt-4o-mini';
    case 'mock':
      return 'simulador';
  }
}

/** Solo OpenAI pide clave hoy. Lo decide el backend; aquí solo se dibuja. */
export function needsApiKey(kind: ProviderKind): boolean {
  return kind === 'open_ai';
}

/** El simulador no habla con ningún servidor: no tiene URL ni lista de modelos. */
export function hasServer(kind: ProviderKind): boolean {
  return kind !== 'mock';
}
