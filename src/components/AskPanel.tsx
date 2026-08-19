import { useCallback, useState } from 'react';
import { ask } from '@/ipc/commands';
import { describeError } from '@/hooks/useAsync';
import type {
  AnswerEvent,
  AnswerStyle,
  DroppedCitation,
  FragmentSummary,
  UnsupportedDetail,
  VerifiedCitation,
} from '@/ipc/types';

const STYLE_LABELS: Record<AnswerStyle, string> = {
  behavioral: 'Comportamental (STAR)',
  technical: 'Técnica',
  general: 'General',
};

/**
 * Detectar el tipo de pregunta automáticamente es §7 y llega en la Fase 5. Hasta
 * entonces se elige a mano: es una casilla menos que fingir que ya funciona.
 */
const STYLES: readonly AnswerStyle[] = ['behavioral', 'technical', 'general'];

type State =
  | { readonly status: 'idle' }
  | { readonly status: 'working'; readonly preview: string; readonly context: Context | null }
  | {
      readonly status: 'answered';
      readonly context: Context | null;
      readonly answer: string;
      readonly keyPoints: readonly string[];
      readonly followUps: readonly string[];
      readonly citations: readonly VerifiedCitation[];
      readonly dropped: readonly DroppedCitation[];
      readonly elapsedMs: number;
    }
  | {
      readonly status: 'unsupported';
      readonly context: Context | null;
      readonly explanation: string;
      readonly detail: UnsupportedDetail;
      readonly structure: readonly string[];
    }
  | { readonly status: 'failed'; readonly message: string };

interface Context {
  readonly fragments: readonly FragmentSummary[];
  /** URL a la que se envió. Cadena vacía si no salió del equipo. */
  readonly sentTo: string;
}

export function AskPanel({ projectId }: { readonly projectId: number | null }) {
  const [question, setQuestion] = useState('');
  const [style, setStyle] = useState<AnswerStyle>('behavioral');
  const [state, setState] = useState<State>({ status: 'idle' });

  const onAsk = useCallback(() => {
    if (projectId === null || question.trim() === '') return;

    setState({ status: 'working', preview: '', context: null });

    const onEvent = (event: AnswerEvent) => {
      setState((current) => reduce(current, event));
    };

    ask(projectId, question, style, onEvent).catch((cause: unknown) => {
      setState({ status: 'failed', message: describeError(cause) });
    });
  }, [projectId, question, style]);

  const context = 'context' in state ? state.context : null;

  return (
    <section className="card">
      <h2>Preguntar</h2>
      <p className="muted">
        Escribe una pregunta de entrevista. La respuesta solo aparece si el modelo la
        respalda con una cita literal de tus documentos; si no, verás el aviso en lugar de
        la respuesta.
      </p>

      <div className="row">
        <select
          value={style}
          onChange={(event) => {
            setStyle(event.target.value as AnswerStyle);
          }}
        >
          {STYLES.map((id) => (
            <option key={id} value={id}>
              {STYLE_LABELS[id]}
            </option>
          ))}
        </select>
        <input
          className="grow"
          value={question}
          placeholder="Cuéntame un proyecto complicado que hayas hecho"
          onChange={(event) => {
            setQuestion(event.target.value);
          }}
          onKeyDown={(event) => {
            if (event.key === 'Enter') onAsk();
          }}
        />
        <button
          type="button"
          className="btn"
          disabled={state.status === 'working' || projectId === null}
          onClick={onAsk}
        >
          {state.status === 'working' ? 'Generando…' : 'Responder'}
        </button>
      </div>

      {context !== null && context.sentTo !== '' && (
        <p className="notice notice--cloud">
          <strong>Procesamiento en la nube activo.</strong> Se han enviado a{' '}
          <code>{context.sentTo}</code> la pregunta y los {context.fragments.length}{' '}
          fragmentos de abajo. El resto de tus documentos no ha salido de este equipo.
        </p>
      )}

      {state.status === 'working' && (
        <>
          {state.context === null ? (
            <p className="muted">Buscando en tus documentos…</p>
          ) : state.preview === '' ? (
            <p className="muted">Verificando las citas antes de mostrar nada…</p>
          ) : (
            <p className="answer answer--streaming">{state.preview}</p>
          )}
        </>
      )}

      {state.status === 'answered' && (
        <>
          <p className="answer">{state.answer}</p>

          {state.keyPoints.length > 0 && (
            <>
              <h3>Puntos clave</h3>
              <ul className="reasons">
                {state.keyPoints.map((point) => (
                  <li key={point}>{point}</li>
                ))}
              </ul>
            </>
          )}

          {state.followUps.length > 0 && (
            <>
              <h3>Posibles repreguntas</h3>
              <ul className="reasons">
                {state.followUps.map((followUp) => (
                  <li key={followUp}>{followUp}</li>
                ))}
              </ul>
            </>
          )}

          <h3>De dónde sale</h3>
          <ul className="citations">
            {state.citations.map((citation) => (
              <li key={`${String(citation.chunkId)}-${citation.quote}`}>
                <span className="results__meta">
                  {citation.documentTitle} · fragmento {citation.fragment}
                </span>
                <q>{citation.quote}</q>
              </li>
            ))}
          </ul>

          <p className="muted small">
            {state.elapsedMs} ms de búsqueda más generación (no incluye cargar el modelo de
            embeddings, que en una entrevista ya está en memoria)
            {state.dropped.length > 0 &&
              ` · ${String(state.dropped.length)} cita(s) descartadas por no ser literales`}
            .
          </p>
        </>
      )}

      {state.status === 'unsupported' && (
        <>
          <p className="notice notice--warn">⚠ {state.explanation}</p>
          <h3>Cómo estructurar la respuesta tú</h3>
          <ul className="reasons">
            {state.structure.map((step) => (
              <li key={step}>{step}</li>
            ))}
          </ul>
          <p className="muted small">
            Esta guía la escribe la aplicación, no el modelo: cuando no hay experiencia
            que citar, pedirle una plantilla es invitarle a rellenarla.
          </p>
        </>
      )}

      {state.status === 'failed' && <p className="error">{state.message}</p>}

      {context !== null && context.fragments.length > 0 && (
        <details className="fragments">
          <summary className="muted small">
            Fragmentos enviados al modelo ({context.fragments.length})
          </summary>
          <ol className="results">
            {context.fragments.map((fragment) => (
              <li key={fragment.number}>
                <div className="results__meta">{fragment.documentTitle}</div>
                <p>{fragment.preview}</p>
              </li>
            ))}
          </ol>
        </details>
      )}
    </section>
  );
}

/**
 * Los eventos llegan en orden y cada uno solo puede avanzar el estado. `delta` ya viene
 * verificado desde Rust: si esto recibe texto, es que hay con qué respaldarlo.
 */
function reduce(current: State, event: AnswerEvent): State {
  const context = 'context' in current ? current.context : null;

  switch (event.event) {
    case 'retrieved':
      return {
        status: 'working',
        preview: '',
        context: { fragments: event.fragments, sentTo: event.sentTo },
      };
    case 'delta':
      return {
        status: 'working',
        preview: (current.status === 'working' ? current.preview : '') + event.text,
        context,
      };
    case 'completed':
      return {
        status: 'answered',
        context,
        answer: event.answer,
        keyPoints: event.keyPoints,
        followUps: event.followUps,
        citations: event.citations,
        dropped: event.dropped,
        elapsedMs: event.elapsedMs,
      };
    case 'unsupported':
      return {
        status: 'unsupported',
        context,
        explanation: event.explanation,
        detail: event.detail,
        structure: event.structure,
      };
    case 'failed':
      return { status: 'failed', message: event.message };
  }
}
