import { useCallback, useState } from 'react';
import { open } from '@tauri-apps/plugin-dialog';
import {
  deleteDocument,
  importDocument,
  listDocuments,
  listProjects,
  searchContext,
} from '@/ipc/commands';
import { describeError, useAsync } from '@/hooks/useAsync';
import { AskPanel } from '@/components/AskPanel';
import { ModelCard } from '@/components/ModelCard';
import { DOCUMENT_KINDS, type DocumentKind, type Retrieval } from '@/ipc/types';

const KIND_LABELS: Record<DocumentKind, string> = {
  cv: 'CV',
  job_offer: 'Oferta de empleo',
  company: 'Info de la empresa',
  notes: 'Notas personales',
  prepared_answers: 'Respuestas preparadas',
  other: 'Otro',
};

type Busy = { readonly kind: 'idle' } | { readonly kind: 'working'; readonly what: string };

export function PrepareView() {
  const projects = useAsync(listProjects);
  const [projectId, setProjectId] = useState<number | null>(null);
  const [kind, setKind] = useState<DocumentKind>('cv');
  const [busy, setBusy] = useState<Busy>({ kind: 'idle' });
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const [question, setQuestion] = useState('');
  const [retrieval, setRetrieval] = useState<Retrieval | null>(null);

  // El proyecto activo: el elegido, o el primero de la lista si aún no se eligió.
  const activeId =
    projectId ??
    (projects.state.status === 'ready' ? (projects.state.value[0]?.id ?? null) : null);

  const documents = useAsync(
    () => (activeId === null ? Promise.resolve([]) : listDocuments(activeId)),
    [activeId],
  );

  const onImport = useCallback(() => {
    if (activeId === null) {
      setError('Crea un proyecto antes de cargar documentos.');
      return;
    }

    setError(null);
    setMessage(null);

    open({
      multiple: false,
      filters: [{ name: 'Documentos', extensions: ['pdf', 'docx', 'txt', 'md'] }],
    })
      .then((selected) => {
        if (typeof selected !== 'string') return;

        setBusy({ kind: 'working', what: 'Extrayendo texto e indexando…' });
        return importDocument(activeId, selected, kind).then((report) => {
          setMessage(
            `Indexado en ${String(report.chunks)} fragmentos${
              report.reindexedFromScratch ? ' (se rehízo el índice: cambió el modelo)' : ''
            }.${
              report.contactDataRemoved > 0
                ? ` ${String(report.contactDataRemoved)} datos de contacto se quedaron fuera del índice.`
                : ''
            }`,
          );
          documents.reload();
        });
      })
      .catch((cause: unknown) => {
        setError(describeError(cause));
      })
      .finally(() => {
        setBusy({ kind: 'idle' });
      });
  }, [activeId, kind, documents]);

  const onSearch = useCallback(() => {
    if (activeId === null) return;

    setError(null);
    setBusy({ kind: 'working', what: 'Buscando…' });

    searchContext(activeId, question)
      .then(setRetrieval)
      .catch((cause: unknown) => {
        setError(describeError(cause));
      })
      .finally(() => {
        setBusy({ kind: 'idle' });
      });
  }, [activeId, question]);

  return (
    <>
      <h1>Preparación</h1>

      <section className="card">
        <h2>Proyecto</h2>
        {projects.state.status === 'loading' && <p className="muted">Cargando…</p>}
        {projects.state.status === 'error' && <p className="error">{projects.state.message}</p>}
        {projects.state.status === 'ready' &&
          (projects.state.value.length === 0 ? (
            <p className="muted">Crea un proyecto en la pestaña Proyectos.</p>
          ) : (
            <select
              value={activeId ?? ''}
              onChange={(event) => {
                setProjectId(Number(event.target.value));
                setRetrieval(null);
              }}
            >
              {projects.state.value.map((project) => (
                <option key={project.id} value={project.id}>
                  {project.name}
                </option>
              ))}
            </select>
          ))}
      </section>

      <section className="card">
        <h2>Base de conocimiento</h2>
        <p className="muted">
          PDF, DOCX, TXT o Markdown. El fichero se lee en tu equipo y solo se guarda aquí; no
          sale a ninguna parte.
        </p>

        <div className="row">
          <select
            value={kind}
            onChange={(event) => {
              setKind(event.target.value as DocumentKind);
            }}
          >
            {DOCUMENT_KINDS.map((id) => (
              <option key={id} value={id}>
                {KIND_LABELS[id]}
              </option>
            ))}
          </select>
          <button
            type="button"
            className="btn"
            disabled={busy.kind === 'working' || activeId === null}
            onClick={onImport}
          >
            Cargar documento
          </button>
        </div>

        {busy.kind === 'working' && <p className="muted">{busy.what}</p>}
        {message !== null && <p className="muted">{message}</p>}
        {error !== null && <p className="error">{error}</p>}

        {documents.state.status === 'ready' && documents.state.value.length > 0 && (
          <ul className="projects">
            {documents.state.value.map((document) => (
              <li key={document.id}>
                <div>
                  <strong>{document.title}</strong>
                  <span className="muted">
                    {KIND_LABELS[document.kind]} · {document.chunkCount} fragmentos
                  </span>
                </div>
                <button
                  type="button"
                  className="btn btn--ghost"
                  onClick={() => {
                    deleteDocument(document.id)
                      .then(() => {
                        documents.reload();
                      })
                      .catch((cause: unknown) => {
                        setError(describeError(cause));
                      });
                  }}
                >
                  Borrar
                </button>
              </li>
            ))}
          </ul>
        )}
      </section>

      <section className="card">
        <h2>Probar la recuperación</h2>
        <p className="muted">
          Escribe una pregunta de entrevista y mira qué fragmentos de tus documentos salen.
          Aquí todavía no interviene ningún modelo de lenguaje: esto es solo la búsqueda.
        </p>

        <div className="row">
          <input
            className="grow"
            value={question}
            placeholder="Cuéntame un proyecto complicado que hayas hecho"
            onChange={(event) => {
              setQuestion(event.target.value);
            }}
            onKeyDown={(event) => {
              if (event.key === 'Enter') onSearch();
            }}
          />
          <button
            type="button"
            className="btn"
            disabled={busy.kind === 'working' || activeId === null}
            onClick={onSearch}
          >
            Buscar
          </button>
        </div>

        {retrieval !== null && (
          <>
            {retrieval.chunks.length === 0 ? (
              <p className="muted">
                No hay nada indexado todavía. Carga un documento primero.
              </p>
            ) : (
              <ol className="results">
                {retrieval.chunks.map((chunk) => (
                  <li key={chunk.id}>
                    <div className="results__meta">
                      {chunk.documentTitle} · fragmento {chunk.ordinal} ·{' '}
                      {chunk.similarity.toFixed(4)}
                    </div>
                    <p>{chunk.text}</p>
                  </li>
                ))}
              </ol>
            )}

            <p className="muted small">
              Despegue del mejor resultado sobre la media: {retrieval.standout.toFixed(4)}
              {retrieval.weakSignal && ' — los resultados apenas se distinguen entre sí'}.
              Es un diagnóstico de la búsqueda, no un juicio sobre si tienes esa experiencia.
            </p>
          </>
        )}
      </section>

      <AskPanel projectId={activeId} />

      <ModelCard />
    </>
  );
}
