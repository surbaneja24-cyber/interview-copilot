import { useCallback, useEffect, useState } from 'react';
import { llmModels } from '@/ipc/commands';
import { describeError } from '@/hooks/useAsync';
import type { LlmSettings } from '@/ipc/types';

interface Props {
  readonly settings: LlmSettings;
  /** Cambia lo que hay en pantalla sin guardar: para lo que se escribe a mano. */
  readonly onEdit: (next: LlmSettings) => void;
  readonly onPersist: (next: LlmSettings) => void;
}

/**
 * URL del servidor y modelo.
 *
 * La lista de modelos no se pide sola: consultarla exige que el servidor esté arrancado,
 * y fallar al abrir Ajustes porque Ollama no está en marcha sería ruido. Se pide cuando
 * el usuario lo pide, y entonces el fallo sí es información.
 */
export function ModelPicker({ settings, onEdit, onPersist }: Props) {
  const [models, setModels] = useState<readonly string[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Los modelos de un proveedor no son los del siguiente. Conservarlos al cambiar sería
  // ofrecer un desplegable de modelos que ese servidor no tiene.
  useEffect(() => {
    setModels(null);
    setMessage(null);
    setError(null);
  }, [settings.kind]);

  const load = useCallback(() => {
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

  // Sin modelo elegido no hay nada que preguntar todavía, así que se pide la lista sola.
  // Es el caso de estrenar el proveedor local: la alternativa era inventar una etiqueta,
  // y eso acababa en un 404 en la primera pregunta.
  useEffect(() => {
    if (settings.model === '') load();
  }, [settings.model, load]);

  return (
    <>
      <label>
        URL del servidor
        <input
          value={settings.baseUrl}
          onChange={(event) => {
            onEdit({ ...settings, baseUrl: event.target.value });
          }}
          onBlur={() => {
            onPersist(settings);
          }}
        />
      </label>

      <label>
        Modelo
        {settings.model === '' && models === null && !busy && (
          <span className="muted small">
            Elige uno de los que tenga tu servidor.
          </span>
        )}
        {models === null ? (
          <input
            value={settings.model}
            onChange={(event) => {
              onEdit({ ...settings, model: event.target.value });
            }}
            onBlur={() => {
              onPersist(settings);
            }}
          />
        ) : (
          <select
            value={settings.model}
            onChange={(event) => {
              onPersist({ ...settings, model: event.target.value });
            }}
          >
            {/* El modelo guardado puede no estar en el servidor: se enseña igual, con el
                aviso, en vez de cambiarlo por las buenas a espaldas del usuario. */}
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

      <button type="button" className="btn btn--ghost" disabled={busy} onClick={load}>
        {busy ? 'Consultando…' : 'Ver qué modelos ofrece el servidor'}
      </button>

      {message !== null && <p className="muted small">{message}</p>}
      {error !== null && <p className="error">{error}</p>}
    </>
  );
}
