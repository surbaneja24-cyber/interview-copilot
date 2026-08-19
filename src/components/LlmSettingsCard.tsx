import { useCallback, useEffect, useState } from 'react';
import {
  apiKeyPresent,
  clearApiKey,
  llmModels,
  llmProviders,
  llmSettings,
  saveLlmSettings,
  setApiKey,
} from '@/ipc/commands';
import { describeError, useAsync } from '@/hooks/useAsync';
import type { LlmSettings, ProviderKind } from '@/ipc/types';

const PROVIDER_LABELS: Record<ProviderKind, string> = {
  local: 'Local (Ollama o llama-server)',
  open_ai: 'OpenAI',
  mock: 'Simulador — sin IA, solo desarrollo',
};

const PROVIDER_NOTES: Record<ProviderKind, string> = {
  local:
    'Nada sale de tu equipo. Necesita Ollama o llama-server arrancado antes de preguntar.',
  open_ai:
    'La pregunta y los fragmentos recuperados se envían a api.openai.com. El audio, el resto de tus documentos y las transcripciones no.',
  mock:
    'Devuelve una respuesta fabricada a partir de tus propios fragmentos, sin consultar ninguna IA. Sirve para comprobar que el flujo funciona; no existe en la versión distribuible.',
};

export function LlmSettingsCard() {
  const providers = useAsync(llmProviders);
  const [settings, setSettings] = useState<LlmSettings | null>(null);
  const [models, setModels] = useState<readonly string[] | null>(null);
  const [hasKey, setHasKey] = useState(false);
  const [keyDraft, setKeyDraft] = useState('');
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    llmSettings()
      .then(setSettings)
      .catch((cause: unknown) => {
        setError(describeError(cause));
      });
  }, []);

  useEffect(() => {
    if (settings === null) return;
    apiKeyPresent(settings.kind)
      .then(setHasKey)
      .catch(() => {
        setHasKey(false);
      });
  }, [settings?.kind]);

  /** Guarda y avisa. Cada cambio se persiste al momento: no hay botón de "aplicar". */
  const persist = useCallback((next: LlmSettings) => {
    setSettings(next);
    setError(null);
    saveLlmSettings(next)
      .then(() => {
        setMessage('Guardado.');
      })
      .catch((cause: unknown) => {
        setError(describeError(cause));
      });
  }, []);

  const onProviderChange = useCallback(
    (kind: ProviderKind) => {
      // Cambiar de proveedor cambia URL y modelo por defecto: conservarlos apuntaría el
      // proveedor nuevo a un servidor que no es el suyo.
      setModels(null);
      setMessage(null);
      persist({
        kind,
        baseUrl: defaultBaseUrl(kind),
        model: defaultModel(kind),
        temperature: settings?.temperature ?? 0.3,
        maxTokens: settings?.maxTokens ?? 800,
        jsonMode: settings?.jsonMode ?? true,
      });
    },
    [persist, settings],
  );

  const onLoadModels = useCallback(() => {
    setBusy(true);
    setError(null);
    setMessage(null);
    llmModels()
      .then((list) => {
        setModels(list);
        setMessage(`${String(list.length)} modelos disponibles.`);
      })
      .catch((cause: unknown) => {
        setError(describeError(cause));
      })
      .finally(() => {
        setBusy(false);
      });
  }, []);

  const onSaveKey = useCallback(() => {
    if (settings === null) return;
    setError(null);
    setApiKey(settings.kind, keyDraft)
      .then(() => {
        setKeyDraft('');
        setHasKey(true);
        setMessage('Clave guardada en el almacén de credenciales de Windows.');
      })
      .catch((cause: unknown) => {
        setError(describeError(cause));
      });
  }, [settings, keyDraft]);

  const onClearKey = useCallback(() => {
    if (settings === null) return;
    clearApiKey(settings.kind)
      .then(() => {
        setHasKey(false);
        setMessage('Clave borrada.');
      })
      .catch((cause: unknown) => {
        setError(describeError(cause));
      });
  }, [settings]);

  if (settings === null) {
    return (
      <section className="card">
        <h2>Modelo de lenguaje</h2>
        {error === null ? <p className="muted">Cargando…</p> : <p className="error">{error}</p>}
      </section>
    );
  }

  return (
    <section className="card">
      <h2>Modelo de lenguaje</h2>

      <div className="form">
        <label>
          Proveedor
          <select
            value={settings.kind}
            onChange={(event) => {
              onProviderChange(event.target.value as ProviderKind);
            }}
          >
            {(providers.state.status === 'ready'
              ? providers.state.value
              : ([settings.kind] as readonly ProviderKind[])
            ).map((kind) => (
              <option key={kind} value={kind}>
                {PROVIDER_LABELS[kind]}
              </option>
            ))}
          </select>
        </label>

        <p className={settings.kind === 'open_ai' ? 'notice notice--cloud' : 'muted small'}>
          {settings.kind === 'open_ai' && <strong>Cloud processing enabled. </strong>}
          {PROVIDER_NOTES[settings.kind]}
        </p>

        {settings.kind !== 'mock' && (
          <>
            <label>
              URL del servidor
              <input
                value={settings.baseUrl}
                onChange={(event) => {
                  setSettings({ ...settings, baseUrl: event.target.value });
                }}
                onBlur={() => {
                  persist(settings);
                }}
              />
            </label>

            <label>
              Modelo
              {models === null ? (
                <input
                  value={settings.model}
                  onChange={(event) => {
                    setSettings({ ...settings, model: event.target.value });
                  }}
                  onBlur={() => {
                    persist(settings);
                  }}
                />
              ) : (
                <select
                  value={settings.model}
                  onChange={(event) => {
                    persist({ ...settings, model: event.target.value });
                  }}
                >
                  {models.includes(settings.model) ? null : (
                    <option value={settings.model}>{settings.model} (no está en la lista)</option>
                  )}
                  {models.map((model) => (
                    <option key={model} value={model}>
                      {model}
                    </option>
                  ))}
                </select>
              )}
            </label>

            <button type="button" className="btn btn--ghost" disabled={busy} onClick={onLoadModels}>
              {busy ? 'Consultando…' : 'Ver qué modelos ofrece el servidor'}
            </button>
          </>
        )}

        {settings.kind === 'open_ai' && (
          <>
            <h3>Clave de API</h3>
            <p className="muted small">
              Se guarda en el Administrador de credenciales de Windows, no en la base de
              datos de la aplicación. No hay forma de volver a mostrarla desde aquí: solo
              sustituirla o borrarla.
            </p>
            <div className="row">
              <input
                className="grow"
                type="password"
                autoComplete="off"
                placeholder={hasKey ? '•••••••• (hay una guardada)' : 'sk-…'}
                value={keyDraft}
                onChange={(event) => {
                  setKeyDraft(event.target.value);
                }}
              />
              <button type="button" className="btn" disabled={keyDraft === ''} onClick={onSaveKey}>
                Guardar
              </button>
              {hasKey && (
                <button type="button" className="btn btn--ghost" onClick={onClearKey}>
                  Borrar
                </button>
              )}
            </div>
          </>
        )}
      </div>

      {message !== null && <p className="muted small">{message}</p>}
      {error !== null && <p className="error">{error}</p>}
    </section>
  );
}

/**
 * Espejo de `ProviderKind::default_base_url` en Rust. Está duplicado a sabiendas: la
 * alternativa era un viaje más al backend solo para rellenar un campo al cambiar de
 * proveedor, y el valor se persiste igualmente, así que el backend sigue mandando.
 */
function defaultBaseUrl(kind: ProviderKind): string {
  switch (kind) {
    case 'local':
      return 'http://localhost:11434/v1';
    case 'open_ai':
      return 'https://api.openai.com/v1';
    case 'mock':
      return '(ninguno)';
  }
}

function defaultModel(kind: ProviderKind): string {
  switch (kind) {
    case 'local':
      return 'qwen2.5:3b-instruct';
    case 'open_ai':
      return 'gpt-4o-mini';
    case 'mock':
      return 'simulador';
  }
}
