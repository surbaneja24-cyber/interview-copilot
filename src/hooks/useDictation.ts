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

  // Qué intento de dictado manda. Abrir el micrófono son dos viajes al backend, y en ese
  // rato pueden llegar otro `start` o un `stop`; sin esto, el que arrancó primero termina
  // igualmente y monta su temporizador encima del bueno.
  //
  // No es un caso raro: `StrictMode` monta, desmonta y vuelve a montar cada efecto en
  // desarrollo, así que el modo diapositiva abre el micrófono dos veces en cada pregunta
  // —medido el 2026-08-20, dos `capturando micrófono` en el mismo segundo—. El temporizador
  // que quedaba huérfano seguía preguntando por transcripciones después de parar, y
  // compartía `before` con el vivo: los dos veían los mismos turnos como nuevos.
  const attempt = useRef(0);
  /// Si se ha llegado a pedir el micrófono. Ver `stop`.
  const opened = useRef(false);

  const cancelTimer = useCallback(() => {
    if (timer.current !== null) {
      window.clearInterval(timer.current);
      timer.current = null;
    }
  }, []);

  const stop = useCallback(() => {
    // Invalida cualquier arranque que siga en vuelo, además del temporizador ya montado.
    attempt.current += 1;
    cancelTimer();
    setDictating(false);

    // Solo se manda cerrar si se llegó a mandar abrir. Parece de más y no lo es: el
    // desmontaje de `StrictMode` para un dictado que aún no había pedido el micrófono, y
    // ese cierre llegaría al backend **después** del `start_capture` del montaje bueno.
    // El resultado sería la pantalla diciendo "escuchando" con el micrófono cerrado, que
    // es justo el fallo que no se puede tener: no da error y no escribe nada.
    if (opened.current) {
      opened.current = false;
      void stopCapture('mic');
    }
  }, [cancelTimer]);

  // Salir de la pantalla suelta el micrófono. Dejarlo abierto porque el usuario cambió de
  // pestaña sería seguir escuchando sin decirlo.
  useEffect(() => stop, [stop]);

  const start = useCallback(() => {
    setError(null);

    attempt.current += 1;
    const mine = attempt.current;
    // Un arranque nuevo se lleva por delante el temporizador del anterior aunque el
    // micrófono siga abierto: `start_capture` ya suelta la captura previa por su cuenta.
    cancelTimer();

    transcript()
      .then((current) => {
        // Otro `start` o un `stop` llegaron mientras se preguntaba: abrir el micrófono
        // ahora significaría soltar la captura buena para volver a abrirla.
        if (attempt.current !== mine) return undefined;
        before.current = current?.entries.length ?? 0;
        // Se marca al pedirlo, no al conseguirlo: si un `stop` llega mientras el backend
        // abre el dispositivo, tiene que mandar cerrarlo igual.
        opened.current = true;
        return startCapture('mic', null);
      })
      .then(() => {
        if (attempt.current !== mine) return;
        setDictating(true);
        timer.current = window.setInterval(() => {
          transcript()
            .then((current) => {
              // La consulta salió antes de parar y vuelve después: ese texto es de un
              // dictado que ya no existe, y soltarlo lo escribiría en la pregunta
              // siguiente.
              if (attempt.current !== mine) return;

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
        if (attempt.current !== mine) return;
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
