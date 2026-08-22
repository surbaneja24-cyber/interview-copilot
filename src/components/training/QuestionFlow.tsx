import { useCallback, useEffect, useRef, useState } from 'react';
import { useDictation } from '@/hooks/useDictation';
import { reviewAnswer } from '@/ipc/commands';
import type { AnswerReview, AnswerReviewReason, TrainingStatus } from '@/ipc/types';

interface Props {
  readonly questions: readonly TrainingStatus[];
  /** Guarda una respuesta. Mientras tanto la pantalla espera, para no perder nada. */
  readonly onAnswer: (question: TrainingStatus, answer: string) => Promise<void>;
  readonly onExit: () => void;
  /** En práctica la pista se esconde: ahí el punto es que no te ayuden. */
  readonly showHints?: boolean;
}

/**
 * Segundos de silencio, ya con el texto en pantalla, antes de pasar a la siguiente.
 *
 * No se avanza al cerrarse el turno del VAD: ese cierre son 700 ms de silencio, que
 * significan "he terminado la frase" y no "he terminado la respuesta". Sumando el cierre
 * del turno, lo que tarda whisper (~2 s aquí) y esta cuenta, hacen falta unos seis segundos
 * callado para que avance. Eso ya es una intención, no una pausa para pensar. Y cualquier
 * cosa que hagas —seguir hablando, escribir, tocar un botón— la cancela.
 */
const SEGUNDOS_PARA_AVANZAR = 4;

/**
 * Por qué se ha parado, en una frase.
 *
 * Dicho como una observación y no como un veredicto: la aplicación no sabe si la respuesta
 * es buena, solo que se parece a las que salieron mal. Quien decide es quien habló.
 */
function motivo(reason: AnswerReviewReason): string {
  switch (reason.kind) {
    case 'nonSpeechMarker':
      return 'Lleva una marca que escribe el transcriptor cuando no oye voz, como [Música].';
    case 'tooShort':
      return `Son ${String(reason.words)} palabras: más corta que cualquier respuesta del banco.`;
    case 'startsMidSentence':
      return 'Empieza a media frase, que es lo que pasa cuando se pierde el principio.';
    case 'dialogueDashes':
      return 'Lleva guiones de diálogo: el transcriptor creyó oír a más de una persona.';
  }
}

type Fase =
  | { readonly tipo: 'respondiendo' }
  | { readonly tipo: 'avanzando'; readonly quedan: number }
  /** Se ha parado a enseñar la respuesta porque se parece a las que salieron mal. */
  | { readonly tipo: 'revisando'; readonly review: AnswerReview }
  | { readonly tipo: 'guardando' }
  | { readonly tipo: 'fin' };

/**
 * Las preguntas de una en una, a pantalla completa, avanzando solas.
 *
 * El objetivo es quitar fricción: contestar veinte preguntas no puede costar veinte
 * decisiones sobre cuál toca ahora, ni veinte clics para abrir el micrófono. Se entra y se
 * habla. Por eso:
 *
 * - Empieza por la primera sin contestar, sin preguntar nada.
 * - El micrófono se abre solo en cada pregunta.
 * - Cuando dejas de hablar del todo, avanza sola —con la cuenta a la vista y cancelable—.
 * - Y aun así se ve lo transcrito antes de guardarlo, porque una respuesta mal transcrita
 *   se queda en el material de todas las entrevistas siguientes.
 *
 * Lo último dejó de ser suficiente el 2026-08-21: verlo pasar no es verlo. Ocho respuestas
 * inservibles se archivaron solas mientras la pantalla las enseñaba. Desde el 22-08 la
 * respuesta pasa por `reviewAnswer` antes de guardarse y, si se parece a las que salieron
 * mal, la cuenta atrás se para y hay que decidir. **Solo entonces**: una respuesta normal
 * sigue guardándose sola, que es el motivo de que este modo exista. El filtro caza siete de
 * las ocho envenenadas y ninguna de las buenas del corpus.
 */
export function QuestionFlow({ questions, onAnswer, onExit, showHints = true }: Props) {
  const primeraSinContestar = Math.max(
    0,
    questions.findIndex((question) => question.answer === null),
  );

  const [index, setIndex] = useState(primeraSinContestar);
  const [text, setText] = useState('');
  const [fase, setFase] = useState<Fase>({ tipo: 'respondiendo' });
  const [saltadas, setSaltadas] = useState<readonly string[]>([]);
  const [error, setError] = useState<string | null>(null);
  const caja = useRef<HTMLTextAreaElement>(null);

  const question = questions[index];

  const recibirDictado = useCallback((trozo: string) => {
    setText((previo) => [previo, trozo].filter(Boolean).join(' '));
    // Hablar cancela la cuenta atrás y la vuelve a empezar desde cero: si sigues, sigue.
    setFase({ tipo: 'avanzando', quedan: SEGUNDOS_PARA_AVANZAR });
  }, []);

  const dictado = useDictation(recibirDictado);
  const { start, stop, dictating } = dictado;

  // Micrófono abierto en cada pregunta, sin pedirlo. Es el clic que más se repetía.
  useEffect(() => {
    start();
    return stop;
  }, [index, start, stop]);

  const irA = useCallback(
    (siguiente: number) => {
      stop();
      setText('');
      setError(null);
      if (siguiente >= questions.length) {
        setFase({ tipo: 'fin' });
      } else {
        setIndex(siguiente);
        setFase({ tipo: 'respondiendo' });
      }
    },
    [questions.length, stop],
  );

  /** Guarda sin más preguntas. Es a lo que lleva el botón de la revisión. */
  const guardar = useCallback(() => {
    const respuesta = text.trim();
    if (respuesta === '' || question === undefined) {
      setFase({ tipo: 'respondiendo' });
      return;
    }

    setFase({ tipo: 'guardando' });
    onAnswer(question, respuesta)
      .then(() => {
        irA(index + 1);
      })
      .catch((cause: unknown) => {
        setError(cause instanceof Error ? cause.message : String(cause));
        setFase({ tipo: 'respondiendo' });
      });
  }, [text, question, onAnswer, index, irA]);

  /**
   * El camino normal: mirar la respuesta y, solo si se parece a las que salieron mal,
   * pararse a preguntar.
   *
   * Pasa por aquí tanto la cuenta atrás como el botón de guardar. El botón también, porque
   * una respuesta envenenada guardada por un clic impaciente envenena igual, y la
   * confirmación es una sola.
   */
  const guardarYSeguir = useCallback(() => {
    const respuesta = text.trim();
    // Sin nada que guardar no se avanza: pasa si el micrófono se abrió y no se dijo nada
    // inteligible. Volver a "respondiendo" evita quedarse con una cuenta atrás a cero.
    if (respuesta === '' || question === undefined) {
      setFase({ tipo: 'respondiendo' });
      return;
    }

    reviewAnswer(respuesta)
      .then((review) => {
        if (review.suspicious) {
          setFase({ tipo: 'revisando', review });
        } else {
          guardar();
        }
      })
      .catch((cause: unknown) => {
        // Si la revisión falla, se para igual. Guardar a ciegas porque la comprobación no
        // contestó sería quitar justamente la red que se acaba de poner.
        setError(cause instanceof Error ? cause.message : String(cause));
        setFase({ tipo: 'respondiendo' });
      });
  }, [text, question, guardar]);

  /** Borra lo transcrito y vuelve a escuchar, sin tocar de pregunta. */
  const repetir = useCallback(() => {
    setText('');
    setError(null);
    setFase({ tipo: 'respondiendo' });
    caja.current?.focus();
  }, []);

  const saltar = useCallback(() => {
    if (question !== undefined) {
      // Saltar no es un fallo: es un hueco, y un hueco es justo lo que hay que trabajar.
      setSaltadas((previas) => [...previas, question.text]);
    }
    irA(index + 1);
  }, [question, index, irA]);

  // La cuenta atrás. Se rehace entera cada segundo para que la pantalla la enseñe.
  useEffect(() => {
    if (fase.tipo !== 'avanzando') return undefined;

    if (fase.quedan <= 0) {
      guardarYSeguir();
      return undefined;
    }

    const id = window.setTimeout(() => {
      setFase({ tipo: 'avanzando', quedan: fase.quedan - 1 });
    }, 1000);
    return () => {
      window.clearTimeout(id);
    };
  }, [fase, guardarYSeguir]);

  // Teclado: para quien escribe, no tener que apuntar con el ratón es la misma idea.
  useEffect(() => {
    const alPulsar = (event: KeyboardEvent) => {
      if (event.key === 'Escape') onExit();
      if (event.key === 'Enter' && (event.ctrlKey || event.metaKey)) guardarYSeguir();
    };
    window.addEventListener('keydown', alPulsar);
    return () => {
      window.removeEventListener('keydown', alPulsar);
    };
  }, [onExit, guardarYSeguir]);

  const cancelarCuenta = useCallback(() => {
    setFase((actual) => (actual.tipo === 'avanzando' ? { tipo: 'respondiendo' } : actual));
  }, []);

  if (fase.tipo === 'fin' || question === undefined) {
    const contestadas = questions.filter((q) => q.answer !== null).length;
    return (
      <section className="card slide">
        <h2>Hecho</h2>
        <p className="muted">
          {contestadas} de {questions.length} contestadas.
          {saltadas.length > 0 &&
            ` Saltaste ${String(saltadas.length)}: son los huecos que la aplicación no podrá
             cubrir durante la entrevista, así que valen más que las que ya sabes.`}
        </p>
        {saltadas.length > 0 && (
          <ul className="reasons">
            {saltadas.map((texto) => (
              <li key={texto}>{texto}</li>
            ))}
          </ul>
        )}
        <div className="model__actions">
          <button type="button" className="btn" onClick={onExit}>
            Volver a la lista
          </button>
        </div>
      </section>
    );
  }

  const hechas = questions.filter((q) => q.answer !== null).length;

  return (
    <section className="card slide">
      <div className="slide__head">
        <span className="muted small">
          {index + 1} de {questions.length} · {hechas} contestadas
        </span>
        <button type="button" className="btn btn--ghost" onClick={onExit}>
          Salir
        </button>
      </div>

      <div className="progress">
        <div
          className="progress__bar"
          style={{ width: `${String(Math.round(((index + 1) / questions.length) * 100))}%` }}
        />
      </div>

      <h2 className="slide__question">{question.text}</h2>
      {showHints && <p className="muted">{question.hint}</p>}

      <textarea
        ref={caja}
        className="answer-box"
        rows={7}
        value={text}
        placeholder="Habla y aparecerá aquí. También puedes escribir."
        onChange={(event) => {
          setText(event.target.value);
          cancelarCuenta();
        }}
      />

      <div className="vad">
        <span className={dictating ? 'vad__speaking' : undefined}>
          {dictating ? 'escuchando' : 'micrófono parado'}
        </span>
        <span>{dictado.status}</span>
      </div>

      {fase.tipo === 'revisando' && (
        <div className="notice notice--warn">
          <strong>Antes de guardarla, míralas.</strong>
          <p className="muted">
            No se ha guardado. Esta respuesta se parece a las que salieron mal al dictar, y
            lo que se guarda aquí es material para todas las entrevistas siguientes.
          </p>
          <ul className="reasons">
            {fase.review.reasons.map((reason) => (
              <li key={reason.kind}>{motivo(reason)}</li>
            ))}
          </ul>
          <div className="model__actions">
            <button type="button" className="btn" onClick={repetir}>
              Repetir la respuesta
            </button>
            <button type="button" className="btn btn--ghost" onClick={guardar}>
              Guardarla igual
            </button>
            <button type="button" className="btn btn--ghost" onClick={saltar}>
              Dejarla para luego
            </button>
          </div>
        </div>
      )}

      {fase.tipo === 'avanzando' && (
        <p className="notice">
          Paso a la siguiente en {fase.quedan} s. Sigue hablando o escribe para quedarte
          aquí.{' '}
          <button type="button" className="btn btn--ghost" onClick={cancelarCuenta}>
            Quedarme
          </button>
        </p>
      )}

      <div className="model__actions">
        <button
          type="button"
          className="btn"
          disabled={text.trim() === '' || fase.tipo === 'guardando' || fase.tipo === 'revisando'}
          onClick={guardarYSeguir}
        >
          {fase.tipo === 'guardando' ? 'Guardando…' : 'Guardar y siguiente'}
        </button>
        <button type="button" className="btn btn--ghost" onClick={saltar}>
          No sé qué contestar
        </button>
        <button type="button" className="btn btn--ghost" onClick={dictating ? stop : start}>
          {dictating ? 'Parar el micrófono' : 'Volver a escuchar'}
        </button>
      </div>

      <p className="muted small">Ctrl+Enter para guardar y seguir · Esc para salir</p>

      {dictado.transcriptError !== null && <p className="error">{dictado.transcriptError}</p>}
      {dictado.error !== null && <p className="error">{dictado.error}</p>}
      {error !== null && <p className="error">{error}</p>}
    </section>
  );
}
