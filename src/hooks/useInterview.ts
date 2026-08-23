import { useCallback, useEffect, useRef, useState } from 'react';
import {
  interviewAsk,
  interviewEnter,
  interviewLeave,
  interviewPoll,
  interviewSuggestion,
  startCapture,
  stopCapture,
  transcript,
} from '@/ipc/commands';
import { describeError } from '@/hooks/useAsync';
import type { AnswerEvent, InterviewView, Sugerencia, TranscriptState } from '@/ipc/types';

/**
 * Cada cuánto se le pregunta al backend por la entrevista.
 *
 * La máquina no tiene reloj propio: avanza cuando alguien mira (§5.5). Doscientos
 * milisegundos es la mitad de lo que tarda el VAD en abrir un turno, así que la pantalla
 * nunca va más de un tic por detrás de lo que está pasando.
 */
const POLL_MS = 200;

const PARADA: InterviewView = { state: 'off', skipped: 0, pendingQuestion: null };

/**
 * La entrevista en vivo, vista desde la pantalla.
 *
 * Aquí **no hay ninguna regla de la entrevista**: cuándo se prepara una sugerencia, qué
 * turnos cuentan y cuándo se cierra una pregunta están en Rust, en `interview::machine` y
 * `interview::session`, con treinta tests. Esto solo sondea, dibuja y obedece.
 *
 * Lo único que sí decide, porque es de pantalla y no de entrevista:
 *
 * - **No pedir dos sugerencias a la vez.** El sondeo va cada 200 ms y una respuesta tarda
 *   segundos; sin el pestillo se lanzarían decenas de peticiones para la misma pregunta.
 * - **No enseñar la respuesta de la pregunta anterior.** Si el entrevistador amplía la
 *   pregunta mientras se prepara, la petición en vuelo ya no sirve. No se puede cancelar a
 *   mitad, pero sí se puede no enseñar: la sugerencia se guarda con la pregunta a la que
 *   contesta y solo se dibuja si siguen coincidiendo.
 */
export function useInterview(projectId: number | null) {
  const [view, setView] = useState<InterviewView>(PARADA);
  const [sugerencia, setSugerencia] = useState<Sugerencia | null>(null);
  const [error, setError] = useState<string | null>(null);
  // El transcript va en el mismo sondeo: es lo que la pantalla enseña turno a turno, y
  // pedirlo en un temporizador aparte sería tener dos relojes para una sola pantalla.
  const [turnos, setTurnos] = useState<TranscriptState | null>(null);

  /** Hay una petición al modelo en vuelo. */
  const preparando = useRef(false);
  const proyecto = useRef(projectId);
  proyecto.current = projectId;

  const dentro = view.state !== 'off';

  const entrar = useCallback(() => {
    setError(null);
    setSugerencia(null);
    // Las dos capturas: el micrófono para saber cuándo contestas y el loopback para oír al
    // entrevistador. Sin la segunda no hay entrevista, solo un dictado caro.
    Promise.all([startCapture('mic', null), startCapture('system', null)])
      .then(() => interviewEnter())
      .then(setView)
      .catch((cause: unknown) => {
        setError(describeError(cause));
      });
  }, []);

  const salir = useCallback(() => {
    interviewLeave()
      .then(setView)
      .catch((cause: unknown) => {
        setError(describeError(cause));
      })
      .finally(() => {
        void stopCapture('mic');
        void stopCapture('system');
      });
  }, []);

  // Salir de la pantalla cierra la entrevista y suelta los dispositivos. Dejarla abierta
  // porque el usuario cambió de pestaña sería seguir escuchando sin decirlo.
  useEffect(() => salir, [salir]);

  useEffect(() => {
    if (!dentro) return undefined;

    const id = window.setInterval(() => {
      transcript()
        .then(setTurnos)
        .catch(() => {
          // Diagnóstico: que falle no puede parar la entrevista.
        });

      interviewPoll(false)
        .then((actual) => {
          setView(actual);

          if (actual.state !== 'preparing' || preparando.current) return undefined;
          const id = proyecto.current;
          if (id === null) return undefined;

          preparando.current = true;
          const pregunta = actual.question;
          return interviewAsk(id, (event) => {
            setSugerencia((previa) => aplicar(previa, pregunta, event));
          })
            .then((preparada) => {
              // `false` significa que no había nada que preparar, y estando en `preparing`
              // eso solo puede pasar si la petición anterior se perdió por el camino. Sin
              // esto la entrevista se quedaría en "preparando" hasta que alguien saliera, y
              // en pantalla eso se parece demasiado a que el modelo va lento.
              //
              // Se cierra así y no con un tiempo de espera porque no hace falta inventar
              // ningún número: que no haya pregunta pendiente ya es la prueba.
              if (!preparada) return interviewSuggestion(false).then(setView);
              return undefined;
            })
            .catch((cause: unknown) => {
              setError(describeError(cause));
            })
            .finally(() => {
              preparando.current = false;
            });
        })
        .catch(() => {
          // Un sondeo que falla no puede parar la entrevista: el siguiente va en 200 ms.
        });
    }, POLL_MS);

    return () => {
      window.clearInterval(id);
    };
  }, [dentro]);

  /** La pregunta que se está contestando ahora, si la hay. */
  const pregunta =
    view.state === 'preparing' || view.state === 'suggesting' || view.state === 'answering'
      ? view.question
      : null;

  return {
    view,
    dentro,
    pregunta,
    turnos,
    /** La sugerencia, **solo si es de la pregunta de ahora**. */
    sugerencia: sugerencia !== null && sugerencia.pregunta === pregunta ? sugerencia : null,
    entrar,
    salir,
    error,
  } as const;
}

/** Va montando la sugerencia con lo que llega por el canal. */
function aplicar(previa: Sugerencia | null, pregunta: string, event: AnswerEvent): Sugerencia {
  const base: Sugerencia =
    previa?.pregunta === pregunta ? previa : { pregunta, texto: '', keyPoints: [], followUps: [] };

  switch (event.event) {
    case 'retrieved':
      return { ...base, fragmentos: event.fragments.length, sentTo: event.sentTo };
    case 'delta':
      return { ...base, texto: base.texto + event.text };
    case 'completed':
      return {
        ...base,
        texto: event.answer,
        keyPoints: event.keyPoints,
        followUps: event.followUps,
        citas: event.citations.length,
        elapsedMs: event.elapsedMs,
      };
    case 'unsupported':
      return { ...base, texto: '', sinMaterial: event.explanation };
    case 'failed':
      return { ...base, fallo: event.message };
  }
}
