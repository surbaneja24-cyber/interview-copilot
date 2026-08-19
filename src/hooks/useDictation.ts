import { useCallback, useEffect, useRef, useState } from 'react';
import { startCapture, stopCapture, transcript } from '@/ipc/commands';
import { describeError } from '@/hooks/useAsync';
import type { TranscriptState } from '@/ipc/types';

/** Cada cuánto se mira si hay transcripción nueva. */
const POLL_MS = 400;

/**
 * Dictar: abrir el micrófono y recoger lo que se transcriba a partir de ese momento.
 *
 * Vive aquí y no dentro de una pantalla porque lo usan la lista de entrenamiento, el modo
 * diapositiva y —cuando llegue— la práctica. Tres copias de esto acabarían divergiendo en
 * los detalles que importan: qué turnos son de esta respuesta y cuándo se suelta el
 * micrófono.
 *
 * El texto llega **cuando el hablante se calla**, no mientras habla: el turno se cierra con
 * 700 ms de silencio y whisper tarda un par de segundos más. Por eso se expone `state`, con
 * los turnos en cola y los errores: sin eso, la espera se parece demasiado a estar colgado.
 */
export function useDictation(onText: (texto: string) => void) {
  const [dictating, setDictating] = useState(false);
  const [state, setState] = useState<TranscriptState | null>(null);
  const [error, setError] = useState<string | null>(null);

  const timer = useRef<number | null>(null);
  // Cuántos turnos había al empezar: lo que llegue después es lo que se está dictando.
  const before = useRef(0);
  // La función de destino cambia en cada render del que la usa; la referencia evita
  // reiniciar el temporizador por eso.
  const sink = useRef(onText);
  sink.current = onText;

  const stop = useCallback(() => {
    if (timer.current !== null) {
      window.clearInterval(timer.current);
      timer.current = null;
    }
    setDictating(false);
    void stopCapture('mic');
  }, []);

  // Salir de la pantalla suelta el micrófono. Dejarlo abierto porque el usuario cambió de
  // pestaña sería seguir escuchando sin decirlo.
  useEffect(() => stop, [stop]);

  const start = useCallback(() => {
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
              sink.current(nuevos.map((entry) => entry.text).join(' '));
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

  /** Qué está pasando con la transcripción, en una línea para enseñar tal cual. */
  const status =
    state === null
      ? 'Sin transcriptor: falta descargar un modelo de voz en Ajustes → Audio.'
      : [
          state.model,
          state.loaded ? 'modelo cargado' : 'cargando el modelo…',
          state.pending > 0 ? `${String(state.pending)} en cola` : null,
        ]
          .filter(Boolean)
          .join(' · ');

  return {
    dictating,
    start,
    stop,
    status,
    /** Fallo del transcriptor, que no es lo mismo que un fallo al abrir el micrófono. */
    transcriptError: state?.error ?? null,
    error,
  } as const;
}
