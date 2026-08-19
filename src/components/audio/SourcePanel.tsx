import { useCallback, useState } from 'react';
import { audioDevices, startCapture, stopCapture } from '@/ipc/commands';
import { describeError, useAsync } from '@/hooks/useAsync';
import { LevelMeter } from '@/components/audio/LevelMeter';
import { TurnIndicator } from '@/components/audio/TurnIndicator';
import type { CaptureStatus, Source } from '@/ipc/types';

interface Props {
  readonly source: Source;
  readonly title: string;
  readonly explanation: string;
  /** Qué significa en esta fuente que no llegue ni una muestra. No es lo mismo en las dos. */
  readonly silenceHint: string;
  readonly status: CaptureStatus | null;
  readonly onChanged: () => void;
}

/**
 * Una fuente de audio: elegir dispositivo, abrirlo y ver su nivel.
 *
 * El nivel no lo pide este componente. Lo recibe de arriba, donde una sola consulta trae
 * las dos fuentes: dos temporizadores independientes serían el doble de mensajes para
 * dibujar lo mismo.
 */
export function SourcePanel({
  source,
  title,
  explanation,
  silenceHint,
  status,
  onChanged,
}: Props) {
  const devices = useAsync(() => audioDevices(source), [source]);
  const [device, setDevice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const capturing = status?.capturing === true;
  // Abrir el dispositivo y no recibir nada es un fallo distinto de no poder abrirlo, y en
  // pantalla se parecen: los dos enseñan una barra plana.
  const silent = capturing && status.frames === 0;

  const onStart = useCallback(() => {
    setError(null);
    startCapture(source, device)
      .then(onChanged)
      .catch((cause: unknown) => {
        setError(describeError(cause));
      });
  }, [source, device, onChanged]);

  const onStop = useCallback(() => {
    setError(null);
    stopCapture(source)
      .then(onChanged)
      .catch((cause: unknown) => {
        setError(describeError(cause));
      });
  }, [source, onChanged]);

  return (
    <div className="source">
      <div className="model__head">
        <h3>{title}</h3>
        <span className={`status ${capturing ? 'status--on' : ''}`}>
          <i />
          {capturing ? 'escuchando' : 'parado'}
        </span>
      </div>

      <p className="muted small">{explanation}</p>

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
        <p className="muted">No hay ningún dispositivo para esta fuente.</p>
      )}

      <LevelMeter level={status?.level ?? null} />
      <TurnIndicator vad={status?.vad ?? null} />

      <div className="model__actions">
        <button type="button" className="btn" onClick={capturing ? onStop : onStart}>
          {capturing ? 'Parar' : 'Escuchar'}
        </button>
        <button
          type="button"
          className="btn btn--ghost"
          disabled={capturing}
          onClick={devices.reload}
        >
          Volver a buscar
        </button>
      </div>

      {status !== null && status.capturing && (
        <p className="muted small">
          {status.device} — {status.sampleRate} Hz, {status.channels} canales,{' '}
          {status.frames.toLocaleString('es-ES')} muestras.
        </p>
      )}
      {silent && <p className="notice notice--warn">{silenceHint}</p>}
      {status?.error != null && <p className="error">{status.error}</p>}
      {error !== null && <p className="error">{error}</p>}
    </div>
  );
}
