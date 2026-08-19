import { useCallback, useState } from 'react';
import { downloadSttModel, sttModels } from '@/ipc/commands';
import { describeError, useAsync } from '@/hooks/useAsync';

/**
 * Los modelos de transcripción, con su tamaño y cuál recomienda el hardware.
 *
 * Se descargan de uno en uno y solo cuando se pide: son entre 75 y 490 MB, y §2 dice que la
 * aplicación no depende de la red. Se enseña el tamaño porque en un portátil con 5,7 GB
 * útiles, elegir el modelo grande no es gratis y el usuario tiene derecho a saberlo antes.
 */
export function SttModelPicker() {
  const models = useAsync(sttModels);
  const [downloading, setDownloading] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const onDownload = useCallback(
    (id: string) => {
      setDownloading(id);
      setError(null);
      downloadSttModel(id)
        .then(() => {
          models.reload();
        })
        .catch((cause: unknown) => {
          setError(describeError(cause));
        })
        .finally(() => {
          setDownloading(null);
        });
    },
    [models],
  );

  if (models.state.status !== 'ready') {
    return models.state.status === 'error' ? (
      <p className="error">{models.state.message}</p>
    ) : (
      <p className="muted small">Cargando…</p>
    );
  }

  return (
    <>
      <ul className="projects">
        {models.state.value.map((model) => (
          <li key={model.id}>
            <div>
              <strong>{model.id}</strong>
              <span className="muted">
                {(model.bytes / 1024 / 1024).toFixed(0)} MB
                {model.recommended ? ' · recomendado para este equipo' : ''}
                {model.downloaded ? ' · descargado' : ''}
              </span>
            </div>
            {!model.downloaded && (
              <button
                type="button"
                className="btn btn--ghost"
                disabled={downloading !== null}
                onClick={() => {
                  onDownload(model.id);
                }}
              >
                {downloading === model.id ? 'Descargando…' : 'Descargar'}
              </button>
            )}
          </li>
        ))}
      </ul>
      {error !== null && <p className="error">{error}</p>}
    </>
  );
}
