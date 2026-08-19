import { useCallback, useEffect, useRef, useState } from 'react';
import { startCapture, stopCapture, transcript } from '@/ipc/commands';
import { describeError } from '@/hooks/useAsync';
import type { TranscriptState } from '@/ipc/types';

interface Props {
  readonly onSave: (answer: string) => void;
  readonly onCancel: () => void;
}

/** Cada cuánto se mira si hay transcripción nueva mientras se dicta. */
const POLL_MS = 400;

/**
 * Responder escribiendo o hablando.
 *
 * Hablar no es un lujo: una respuesta dictada suena a como habla el candidato, y es esa
 * forma de decirlo la que hace que la sugerencia durante la entrevista suene humana en vez
 * de a currículum leído. Además se contesta en un minuto lo que escribiendo cuesta cinco, y
 * un banco de respuestas a medias no sirve para nada.
 *
 * Se apoya en lo que ya existe: micrófono, VAD y whisper. Aquí solo se recogen los turnos
 * que llegan desde que se pulsó el botón.
 */
export function AnswerBox({ onSave, onCancel }: Props) {
  const [text, setText] = useState('');
  const [dictating, setDictating] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // El estado del transcriptor, para no dejar al usuario mirando un "Escuchando" que no
  // dice si el modelo esta cargando, si hay turnos en cola o si algo ha fallado.
  const [state, setState] = useState<TranscriptState | null>(null);
  const timer = useRef<number | null>(null);
  // Cuántos turnos había antes de empezar: lo que llegue después es esta respuesta.
  const before = useRef(0);

  const stop = useCallback(() => {
    if (timer.current !== null) {
      window.clearInterval(timer.current);
      timer.current = null;
    }
    setDictating(false);
    void stopCapture('mic');
  }, []);

  useEffect(() => stop, [stop]);

  const onDictate = useCallback(() => {
    setError(null);

    transcript()
      .then((current) => {
        before.current = current?.entries.length ?? 0;
        return startCapture('mic', null);
      })
      .then(() => {
        setDictating(true);
        timer.current = window.setInterval(() => {
          transcript()
            .then((current) => {
              setState(current);
              if (current === null) return;
              const nuevos = current.entries.slice(before.current);
              if (nuevos.length === 0) return;

              before.current = current.entries.length;
              setText((previo) =>
                [previo, ...nuevos.map((entry) => entry.text)].filter(Boolean).join(' '),
              );
            })
            .catch(() => {
              // Un fallo al consultar no puede tirar lo que ya se ha dictado.
            });
        }, POLL_MS);
      })
      .catch((cause: unknown) => {
        setError(describeError(cause));
        setDictating(false);
      });
  }, []);

  return (
    <div className="form">
      <textarea
        className="answer-box"
        rows={6}
        value={text}
        placeholder="Escribe tu respuesta, o dale a Dictar y cuéntala en voz alta."
        onChange={(event) => {
          setText(event.target.value);
        }}
      />

      {dictating && (
        <>
          <p className="muted small">
            Escuchando. El texto aparece cuando terminas de hablar, no mientras hablas: hacen
            falta 700 ms de silencio para dar la frase por acabada.
          </p>
          <p className="muted small">
            {state === null
              ? 'Sin transcriptor: falta descargar un modelo de voz en Ajustes → Audio.'
              : [
                  state.model,
                  state.loaded ? 'modelo cargado' : 'cargando el modelo…',
                  state.pending > 0 ? `${String(state.pending)} turnos en cola` : null,
                ]
                  .filter(Boolean)
                  .join(' · ')}
          </p>
          {state?.error != null && <p className="error">{state.error}</p>}
        </>
      )}

      <div className="model__actions">
        <button
          type="button"
          className="btn"
          disabled={text.trim() === ''}
          onClick={() => {
            stop();
            onSave(text.trim());
          }}
        >
          Guardar
        </button>
        <button type="button" className="btn btn--ghost" onClick={dictating ? stop : onDictate}>
          {dictating ? 'Parar de dictar' : 'Dictar'}
        </button>
        <button
          type="button"
          className="btn btn--ghost"
          onClick={() => {
            stop();
            onCancel();
          }}
        >
          Cancelar
        </button>
      </div>

      {error !== null && <p className="error">{error}</p>}
    </div>
  );
}
