import { useCallback, useEffect, useRef, useState } from 'react';
import { loadModel, modelStatus, releaseEmbedder } from '@/ipc/commands';
import { describeError } from '@/hooks/useAsync';
import type { ModelStatus } from '@/ipc/types';

/** Cada cuánto se consulta el estado mientras el modelo se descarga o carga. */
const POLL_MS = 700;

type Phase = 'idle' | 'loading' | 'releasing';

type Display = {
  readonly dot: 'off' | 'busy' | 'on';
  readonly label: string;
  readonly detail: string;
};

export function ModelCard() {
  const [status, setStatus] = useState<ModelStatus | null>(null);
  const [phase, setPhase] = useState<Phase>('idle');
  const [error, setError] = useState<string | null>(null);
  const timer = useRef<number | null>(null);

  const refresh = useCallback(() => {
    modelStatus()
      .then(setStatus)
      .catch((cause: unknown) => {
        setError(describeError(cause));
      });
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  // Mientras algo está en marcha se consulta el estado a intervalos: es la única forma de
  // ver avanzar la descarga, porque fastembed no emite eventos de progreso.
  useEffect(() => {
    if (phase === 'idle') return;

    timer.current = window.setInterval(refresh, POLL_MS);
    return () => {
      if (timer.current !== null) window.clearInterval(timer.current);
      timer.current = null;
    };
  }, [phase, refresh]);

  const run = useCallback(
    (next: Phase, action: () => Promise<void>) => {
      setError(null);
      setPhase(next);
      action()
        .catch((cause: unknown) => {
          setError(describeError(cause));
        })
        .finally(() => {
          setPhase('idle');
          refresh();
        });
    },
    [refresh],
  );

  const downloaded = status?.bytesOnDisk ?? 0;
  const expected = status?.expectedBytes ?? 0;
  const loaded = status?.loaded === true;
  const onDisk = expected > 0 && downloaded >= expected * 0.99;
  const percent = expected > 0 ? Math.min(100, Math.round((downloaded / expected) * 100)) : 0;
  const downloading = phase === 'loading' && !onDisk;

  const display = describe(phase, loaded, onDisk, percent);

  return (
    <section className="card model">
      <header className="model__head">
        <div>
          <h2>Modelo de embeddings</h2>
          <p className="model__id">{status?.modelId ?? '—'}</p>
        </div>
        <span className={`status status--${display.dot}`}>
          <i aria-hidden="true" />
          {display.label}
        </span>
      </header>

      <p className="muted small model__detail">{display.detail}</p>

      {downloading && (
        <div
          className="progress"
          role="progressbar"
          aria-valuenow={percent}
          aria-valuemin={0}
          aria-valuemax={100}
        >
          <div className="progress__bar" style={{ width: `${String(percent)}%` }} />
        </div>
      )}

      <dl className="model__stats">
        <div>
          <dt>En disco</dt>
          <dd>{onDisk ? formatBytes(downloaded) : `${formatBytes(downloaded)} / ${formatBytes(expected)}`}</dd>
        </div>
        <div>
          <dt>En memoria</dt>
          <dd>{loaded ? '≈ 1,1 GB' : '—'}</dd>
        </div>
        <div>
          <dt>Dimensiones</dt>
          <dd>768</dd>
        </div>
      </dl>

      <div className="model__actions">
        <button
          type="button"
          className="btn"
          disabled={phase !== 'idle' || loaded}
          onClick={() => {
            run('loading', loadModel);
          }}
        >
          {phase === 'loading' ? 'Cargando…' : 'Cargar'}
        </button>
        <button
          type="button"
          className="btn btn--ghost"
          disabled={phase !== 'idle' || !loaded}
          onClick={() => {
            run('releasing', releaseEmbedder);
          }}
        >
          {phase === 'releasing' ? 'Liberando…' : 'Liberar memoria'}
        </button>
      </div>

      {error !== null && <p className="error small">{error}</p>}
    </section>
  );
}

function describe(phase: Phase, loaded: boolean, onDisk: boolean, percent: number): Display {
  if (phase === 'releasing') {
    return { dot: 'busy', label: 'Liberando', detail: 'Devolviendo la memoria al sistema.' };
  }
  if (phase === 'loading') {
    return onDisk
      ? {
          dot: 'busy',
          label: 'Iniciando',
          detail: 'El fichero ya está en disco; cargándolo en memoria. Unos segundos.',
        }
      : {
          dot: 'busy',
          label: `Descargando ${String(percent)}%`,
          // La cifra sale de los bytes escritos en disco, no de un evento de la librería.
          detail: 'Descarga única. El porcentaje mide los bytes escritos en disco.',
        };
  }
  if (loaded) {
    return {
      dot: 'on',
      label: 'Listo',
      detail: 'Ocupa memoria mientras esté cargado. Libéralo antes de una entrevista.',
    };
  }
  return onDisk
    ? { dot: 'off', label: 'En reposo', detail: 'Descargado y listo. Se carga solo al indexar o buscar.' }
    : { dot: 'off', label: 'Sin descargar', detail: 'La primera descarga son 1,1 GB y ocurre una sola vez.' };
}

function formatBytes(bytes: number): string {
  if (bytes <= 0) return '0 MB';
  const mb = bytes / (1024 * 1024);
  return mb >= 1024 ? `${(mb / 1024).toFixed(2)} GB` : `${Math.round(mb).toString()} MB`;
}
