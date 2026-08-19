import type { TranscriptState } from '@/ipc/types';

interface Props {
  readonly transcript: TranscriptState | null;
}

const SOURCE_LABELS = {
  mic: 'Tú',
  system: 'Entrevistador',
} as const;

/**
 * Lo transcrito, turno a turno. Es el hito de la fase 4: hablo y el texto aparece.
 *
 * Cada línea lleva cuánto duró el audio y cuánto tardó whisper, y no es un adorno de
 * desarrollo: es el número que decide si el modo LOCAL sirve para una entrevista o solo
 * para practicar (§10). Enseñarlo aquí evita tener que medirlo aparte.
 */
export function TranscriptPanel({ transcript }: Props) {
  if (transcript === null) {
    return (
      <p className="muted small">
        La transcripción arranca con la captura, en cuanto haya un modelo descargado.
      </p>
    );
  }

  return (
    <>
      <div className="vad">
        <span>{transcript.model}</span>
        <span>{transcript.loaded ? 'modelo cargado' : 'modelo sin cargar todavía'}</span>
        {transcript.pending > 0 && <span>{transcript.pending} turnos en cola</span>}
      </div>

      {transcript.entries.length === 0 ? (
        <p className="muted small">
          Nada transcrito aún. El texto aparece cuando alguien termina de hablar, no
          mientras habla: el turno se cierra con 700 ms de silencio.
        </p>
      ) : (
        <ol className="transcript">
          {transcript.entries.map((entry, index) => (
            <li key={`${String(index)}-${entry.text.slice(0, 12)}`}>
              <div className="transcript__meta">
                {SOURCE_LABELS[entry.source]} · {(entry.audioMs / 1000).toFixed(1)} s de audio ·{' '}
                transcrito en {(entry.tookMs / 1000).toFixed(1)} s
              </div>
              <p>{entry.text}</p>
            </li>
          ))}
        </ol>
      )}

      {transcript.error !== null && <p className="error">{transcript.error}</p>}
    </>
  );
}
