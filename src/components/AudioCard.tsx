import { useCallback, useEffect, useRef, useState } from 'react';
import { audioInputs, captureStatus, startCapture, stopCapture } from '@/ipc/commands';
import { describeError, useAsync } from '@/hooks/useAsync';
import type { CaptureStatus } from '@/ipc/types';

/** Cada cuánto se pregunta el nivel mientras hay captura. */
const POLL_MS = 100;

/** Extremo bajo de la barra. Por debajo de −60 dB no hay nada que enseñar. */
const FLOOR_DBFS = -60;

/**
 * Micrófono y medidor de nivel (§11).
 *
 * El medidor no es un adorno: es la única forma de saber, antes de una entrevista, si el
 * dispositivo elegido es el que de verdad está oyendo. Un selector sin medidor deja al
 * usuario descubriendo en mitad de la entrevista que estaba capturando el micrófono de la
 * webcam apagada.
 */
export function AudioCard() {
  const devices = useAsync(audioInputs);
  const [device, setDevice] = useState<string | null>(null);
  const [status, setStatus] = useState<CaptureStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const timer = useRef<number | null>(null);

  const stopPolling = useCallback(() => {
    if (timer.current !== null) {
      window.clearInterval(timer.current);
      timer.current = null;
    }
  }, []);

  // Parar la captura al salir de la pantalla: dejar el micrófono cogido porque el usuario
  // cambió de pestaña sería quedarse escuchando sin decirlo.
  useEffect(() => {
    return () => {
      stopPolling();
      void stopCapture();
    };
  }, [stopPolling]);

  const onStart = useCallback(() => {
    setError(null);
    startCapture(device)
      .then((first) => {
        setStatus(first);
        stopPolling();
        timer.current = window.setInterval(() => {
          captureStatus()
            .then(setStatus)
            .catch((cause: unknown) => {
              setError(describeError(cause));
              stopPolling();
            });
        }, POLL_MS);
      })
      .catch((cause: unknown) => {
        setError(describeError(cause));
        setStatus(null);
      });
  }, [device, stopPolling]);

  const onStop = useCallback(() => {
    stopPolling();
    stopCapture()
      .then(() => {
        setStatus(null);
      })
      .catch((cause: unknown) => {
        setError(describeError(cause));
      });
  }, [stopPolling]);

  const capturing = status?.capturing === true;
  // Abrir el dispositivo y no recibir nada es un fallo distinto de no poder abrirlo, y
  // en pantalla se parecen: los dos enseñan una barra plana.
  const silent = capturing && status.frames === 0;

  return (
    <section className="card">
      <div className="model__head">
        <h2>Micrófono</h2>
        <span className={`status ${capturing ? 'status--on' : ''}`}>
          <i />
          {capturing ? 'MIC' : 'parado'}
        </span>
      </div>

      <p className="muted small">
        El audio del sistema —lo que dice el entrevistador en la videollamada— todavía no se
        captura; llega en el paso siguiente. Esto es solo el micrófono, y sirve para
        comprobar que el dispositivo elegido es el que de verdad oye.
      </p>

      <div className="form">
        <label>
          Dispositivo
          <select
            value={device ?? ''}
            disabled={capturing}
            onChange={(event) => {
              setDevice(event.target.value === '' ? null : event.target.value);
            }}
          >
            <option value="">El que use el sistema por defecto</option>
            {devices.state.status === 'ready' &&
              devices.state.value.map((input) => (
                <option key={input.id} value={input.id}>
                  {input.name}
                  {input.isDefault ? ' (por defecto)' : ''} — {input.sampleRate} Hz,{' '}
                  {input.channels} canales
                </option>
              ))}
          </select>
        </label>
      </div>

      {devices.state.status === 'error' && <p className="error">{devices.state.message}</p>}
      {devices.state.status === 'ready' && devices.state.value.length === 0 && (
        <p className="muted">Este equipo no tiene ninguna entrada de audio.</p>
      )}

      <Meter status={status} />

      <div className="model__actions">
        {capturing ? (
          <button type="button" className="btn" onClick={onStop}>
            Parar
          </button>
        ) : (
          <button type="button" className="btn" onClick={onStart}>
            Escuchar
          </button>
        )}
        <button
          type="button"
          className="btn btn--ghost"
          disabled={capturing}
          onClick={devices.reload}
        >
          Volver a buscar dispositivos
        </button>
      </div>

      {status !== null && (
        <p className="muted small">
          {status.device} — {status.sampleRate} Hz, {status.channels} canales,{' '}
          {status.frames.toLocaleString('es-ES')} muestras.
        </p>
      )}
      {silent && (
        <p className="notice notice--warn">
          El dispositivo abrió pero no está entregando ninguna muestra. Suele ser el
          micrófono silenciado por hardware o el permiso de micrófono de Windows.
        </p>
      )}
      {status?.error != null && <p className="error">{status.error}</p>}
      {error !== null && <p className="error">{error}</p>}
    </section>
  );
}

function Meter({ status }: { readonly status: CaptureStatus | null }) {
  const level = status?.level ?? { rmsDbfs: FLOOR_DBFS, peakDbfs: FLOOR_DBFS };

  return (
    <>
      <div className="meter">
        <div className="meter__bar" style={{ width: `${String(toPercent(level.rmsDbfs))}%` }} />
        <div className="meter__peak" style={{ left: `${String(toPercent(level.peakDbfs))}%` }} />
      </div>
      <div className="meter__scale">
        <span>−60 dB</span>
        <span>
          {status === null
            ? 'sin capturar'
            : `${level.rmsDbfs.toFixed(1)} dB · pico ${level.peakDbfs.toFixed(1)} dB`}
        </span>
        <span>0 dB</span>
      </div>
    </>
  );
}

/** De decibelios a ancho de barra. Lineal en dB, que es como se percibe el volumen. */
function toPercent(dbfs: number): number {
  if (dbfs <= FLOOR_DBFS) return 0;
  if (dbfs >= 0) return 100;
  return Math.round(((dbfs - FLOOR_DBFS) / -FLOOR_DBFS) * 100);
}
