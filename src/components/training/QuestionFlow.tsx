import { useCallback, useEffect, useRef, useState } from 'react';
import { useDictation } from '@/hooks/useDictation';
import { reviewAnswer } from '@/ipc/commands';
import { SEGUNDOS_PARA_AVANZAR, siguienteFase } from '@/components/training/flujo';
import type { Accion, Contexto, Fase } from '@/components/training/flujo';
import type { AnswerReviewReason, TrainingStatus } from '@/ipc/types';

interface Props {
  readonly questions: readonly TrainingStatus[];
  /** Guarda una respuesta. Mientras tanto la pantalla espera, para no perder nada. */
  readonly onAnswer: (question: TrainingStatus, answer: string) => Promise<void>;
  readonly onExit: () => void;
  /** En práctica la pista se esconde: ahí el punto es que no te ayuden. */
  readonly showHints?: boolean;
}

/** Cada cuánto avanza el reloj de la máquina. Es su única fuente de tiempo. */
const TIC_MS = 1000;

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

/**
 * Las preguntas de una en una, a pantalla completa, avanzando solas.
 *
 * El objetivo es quitar fricción: contestar veinte preguntas no puede costar veinte
 * decisiones sobre cuál toca ahora, ni veinte clics para abrir el micrófono. Se entra y se
 * habla.
 *
 * **Aquí no vive ninguna regla.** Cuándo se avanza, cuándo se para y qué hace un trozo de
 * transcripción que llega tarde están en `flujo.ts`, que es una función pura con tests. Esta
 * pantalla solo obedece: dibuja, llama a `tic` una vez por segundo, pide la revisión cuando
 * la máquina lo pide y guarda cuando lo pide.
 *
 * Esa separación se paga sola. Los cinco fallos que ha tenido esta pantalla se encontraron
 * usándola o releyéndola, ninguno con un test, y los cinco eran de cuándo corría un efecto.
 *
 * Lo que sí sigue aquí, porque es de producto y no de estados: una respuesta pasa por
 * `reviewAnswer` antes de guardarse, y si se parece a las que salieron mal hay que decidir.
 * **Solo entonces**: una respuesta normal se sigue guardando sola, que es el motivo de que
 * este modo exista.
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

  const ocupadoRef = useRef(false);
  const textoRef = useRef('');
  textoRef.current = text;

  /**
   * Manda una acción a la máquina con el contexto **de ahora**.
   *
   * Por referencia y no por dependencia: si el contexto entrara como dependencia de los
   * `useCallback`, cada tic recrearía los manejadores y el temporizador. Y sobre todo, leerlo
   * en el momento del despacho es lo que garantiza que se decide con el valor actual, que es
   * de donde salieron tres de los cinco fallos de esta pantalla.
   */
  const despachar = useCallback((accion: Accion) => {
    const ctx: Contexto = {
      ocupado: ocupadoRef.current,
      hayTexto: textoRef.current.trim() !== '',
    };
    setFase((actual) => siguienteFase(actual, accion, ctx));
  }, []);

  const recibirDictado = useCallback(
    (trozo: string) => {
      setText((previo) => {
        const unido = [previo, trozo].filter(Boolean).join(' ');
        textoRef.current = unido;
        return unido;
      });
      despachar({ tipo: 'dictado' });
    },
    [despachar],
  );

  const dictado = useDictation(recibirDictado);
  const { start, stop, dictating } = dictado;
  const ocupado = dictado.speaking || dictado.pending > 0;
  ocupadoRef.current = ocupado;

  // Micrófono abierto en cada pregunta, sin pedirlo. Es el clic que más se repetía.
  useEffect(() => {
    start();
    return stop;
  }, [index, start, stop]);

  // El reloj. Uno solo y siempre el mismo: la máquina decide qué hacer con cada tic.
  useEffect(() => {
    const id = window.setInterval(() => {
      despachar({ tipo: 'tic' });
    }, TIC_MS);
    return () => {
      window.clearInterval(id);
    };
  }, [despachar]);

  const irA = useCallback(
    (siguiente: number) => {
      stop();
      setText('');
      textoRef.current = '';
      setError(null);
      if (siguiente >= questions.length) {
        despachar({ tipo: 'terminar' });
      } else {
        setIndex(siguiente);
        despachar({ tipo: 'reiniciar' });
      }
    },
    [questions.length, stop, despachar],
  );

  // Comprobar la respuesta. La máquina pide entrar aquí; esto obedece y contesta.
  useEffect(() => {
    if (fase.tipo !== 'comprobando') return;

    reviewAnswer(textoRef.current.trim())
      .then((review) => {
        despachar(review.suspicious ? { tipo: 'sospechosa', review } : { tipo: 'limpia' });
      })
      .catch((cause: unknown) => {
        // Guardar a ciegas porque la comprobación no contestó sería quitar justamente la red
        // que se acaba de poner.
        setError(cause instanceof Error ? cause.message : String(cause));
        despachar({ tipo: 'fallo' });
      });
  }, [fase.tipo, despachar]);

  // Guardar. Igual: la máquina lo pide, esto lo hace.
  const guardando = useRef(false);
  useEffect(() => {
    if (fase.tipo !== 'guardando' || question === undefined || guardando.current) return;
    guardando.current = true;

    onAnswer(question, textoRef.current.trim())
      .then(() => {
        irA(index + 1);
      })
      .catch((cause: unknown) => {
        setError(cause instanceof Error ? cause.message : String(cause));
        despachar({ tipo: 'fallo' });
      })
      .finally(() => {
        guardando.current = false;
      });
  }, [fase.tipo, question, onAnswer, index, irA, despachar]);

  const repetir = useCallback(() => {
    setText('');
    textoRef.current = '';
    setError(null);
    despachar({ tipo: 'repetir' });
    caja.current?.focus();
  }, [despachar]);

  const saltar = useCallback(() => {
    if (question !== undefined) {
      // Saltar no es un fallo: es un hueco, y un hueco es justo lo que hay que trabajar.
      setSaltadas((previas) => [...previas, question.text]);
    }
    irA(index + 1);
  }, [question, index, irA]);

  const guardar = useCallback(() => {
    despachar({ tipo: 'guardar' });
  }, [despachar]);

  const quedarme = useCallback(() => {
    despachar({ tipo: 'aMano' });
  }, [despachar]);

  const guardarIgual = useCallback(() => {
    despachar({ tipo: 'limpia' });
  }, [despachar]);

  // Teclado: para quien escribe, no tener que apuntar con el ratón es la misma idea.
  useEffect(() => {
    const alPulsar = (event: KeyboardEvent) => {
      if (event.key === 'Escape') onExit();
      if (event.key === 'Enter' && (event.ctrlKey || event.metaKey)) guardar();
    };
    window.addEventListener('keydown', alPulsar);
    return () => {
      window.removeEventListener('keydown', alPulsar);
    };
  }, [onExit, guardar]);

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
  const esperandoAlgo = ocupado && text !== '';

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
          textoRef.current = event.target.value;
          despachar({ tipo: 'aMano' });
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
            No se ha guardado. Esta respuesta se parece a las que salieron mal al dictar, y lo
            que se guarda aquí es material para todas las entrevistas siguientes.
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
            <button type="button" className="btn btn--ghost" onClick={guardarIgual}>
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
          Paso a la siguiente en {fase.quedan} s. Sigue hablando o escribe para quedarte aquí.{' '}
          <button type="button" className="btn btn--ghost" onClick={quedarme}>
            Quedarme
          </button>
        </p>
      )}

      {(fase.tipo === 'respondiendo' || fase.tipo === 'quieto') && esperandoAlgo && (
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
          disabled={
            text.trim() === '' ||
            fase.tipo === 'guardando' ||
            fase.tipo === 'comprobando' ||
            fase.tipo === 'revisando'
          }
          onClick={guardar}
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

      <p className="muted small">
        Ctrl+Enter para guardar y seguir · Esc para salir · avanza sola tras{' '}
        {SEGUNDOS_PARA_AVANZAR} s en silencio
      </p>

      {dictado.transcriptError !== null && <p className="error">{dictado.transcriptError}</p>}
      {dictado.error !== null && <p className="error">{dictado.error}</p>}
      {error !== null && <p className="error">{error}</p>}
    </section>
  );
}
