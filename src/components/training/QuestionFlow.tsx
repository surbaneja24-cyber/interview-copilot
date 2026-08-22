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
 * significan "he terminado la frase" y no "he terminado la respuesta". Lo que se busca es
 * que hagan falta **unos seis segundos callado** para que avance, porque eso ya es una
 * intención y no una pausa para pensar.
 *
 * Eran cuatro segundos, y esa cuenta se apoyaba en una estimación: "lo que tarda whisper
 * (~2 s aquí)". Medido el 2026-08-22 (§4.7), tarda entre 2,4 y 3,8 s, y encima casi
 * independientemente de lo que dure el turno. Con el cierre del VAD y el sondeo, el total
 * real eran unos ocho segundos, no seis. Bajar a dos devuelve la cuenta a lo que siempre
 * quiso ser:
 *
 *     0,7 s de cierre del VAD + ~3,0 s de whisper + 0,4 s de sondeo + 2 s = ~6,1 s
 *
 * El número no se ha elegido más corto porque sí: se ha recalculado con la medición que
 * faltaba cuando se puso.
 */
const SEGUNDOS_PARA_AVANZAR = 2;

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

  /**
   * Si hay algo en marcha: alguien hablando, o audio que whisper todavía no ha devuelto.
   *
   * Las dos condiciones tienen el mismo motivo y ninguna lleva número dentro: mientras
   * cualquiera de ellas sea cierta, **la respuesta que hay en pantalla no está completa**, y
   * guardarla es archivar media respuesta.
   *
   * `pending` es la que no se veía a simple vista. El turno se cierra 700 ms después de
   * callarse y whisper tarda otros tres segundos (§4.7), así que hay un hueco largo en el
   * que ya no se oye nada y todavía falta texto por llegar.
   */
  const ocupado = dictado.speaking || dictado.pending > 0;

  // La cuenta atrás. Se rehace entera cada segundo para que la pantalla la enseñe.
  useEffect(() => {
    if (fase.tipo !== 'avanzando') return undefined;

    // Se mira el valor de **ahora**, en cada tic, y no el momento en que cambió.
    //
    // La primera versión de esto era un efecto disparado por el cambio de `speaking`, y no
    // servía: con 3,5 s de retraso en la transcripción, el texto de una frase llega cuando
    // ya has empezado la siguiente. `speaking` llevaba rato en `true`, no cambiaba, el
    // efecto no se ejecutaba y la cuenta corría hasta el final. Cortaba a mitad de
    // respuesta, que es exactamente lo que fue a arreglar.
    if (ocupado) {
      setFase({ tipo: 'respondiendo' });
      return undefined;
    }

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
  }, [fase, guardarYSeguir, ocupado]);

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
          {dictado.pending > 0
            ? `transcribiendo${dictado.pending > 1 ? ` (${String(dictado.pending)} en cola)` : '…'}`
            : dictating
              ? 'escuchando'
              : 'micrófono parado'}
        </span>
        {/* Lo que costó el último turno, a la vista. Ya se medía y solo se veía en Ajustes,
            que es donde no está quien espera. */}
        {dictado.lastTurn !== null && (
          <span>
            último turno {(dictado.lastTurn.audioMs / 1000).toFixed(1)} s →{' '}
            {(dictado.lastTurn.tookMs / 1000).toFixed(1)} s en transcribir
          </span>
        )}
        {dictado.discarded > 0 && (
          <span>
            {dictado.discarded} turno{dictado.discarded === 1 ? '' : 's'} descartado
            {dictado.discarded === 1 ? '' : 's'} por cortos
          </span>
        )}
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

      {fase.tipo === 'respondiendo' && ocupado && text !== '' && (
        <p className="muted small">
          {dictado.pending > 0
            ? 'Esperando al resto de lo que has dicho antes de contar para avanzar.'
            : 'Te sigo oyendo, la cuenta para avanzar está parada.'}
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
