import { useState } from 'react';
import { listProjects } from '@/ipc/commands';
import { useAsync } from '@/hooks/useAsync';
import { useInterview } from '@/hooks/useInterview';
import { TranscriptPanel } from '@/components/audio/TranscriptPanel';
import type { InterviewView as Vista } from '@/ipc/types';

/** Lo que la pantalla dice que está pasando, en dos palabras. */
const ESTADOS: Record<Vista['state'], string> = {
  off: 'fuera de la entrevista',
  waiting: 'escuchando',
  asking: 'el entrevistador está hablando',
  preparing: 'preparando la sugerencia',
  suggesting: 'sugerencia lista',
  answering: 'estás contestando',
};

/**
 * La entrevista en vivo (§9).
 *
 * Es la pantalla más delgada del proyecto a propósito. Todo lo que puede equivocarse
 * —cuándo se prepara algo, qué turnos cuentan, con qué material y en qué estilo— está en
 * Rust y tiene tests. Aquí solo se dibuja lo que el backend dice que está pasando.
 *
 * Dos decisiones de pantalla que sí son decisiones:
 *
 * - **La pregunta se enseña siempre encima de la sugerencia.** Una respuesta sin la pregunta
 *   delante no se puede juzgar, y quien la va a leer de reojo en mitad de una videollamada
 *   necesita saber en un vistazo si la app entendió lo que le preguntaron.
 * - **Los avisos de §6 no se esconden.** Que el modelo diga que no hay material es
 *   información, no un fallo: significa que ahí no hay nada preparado y toca improvisar,
 *   que es justo lo que hay que saber en ese momento y no después.
 */
export function InterviewView() {
  const projects = useAsync(listProjects);
  const [projectId, setProjectId] = useState<number | null>(null);

  const activeId =
    projectId ??
    (projects.state.status === 'ready' ? (projects.state.value[0]?.id ?? null) : null);

  const { view, dentro, pregunta, turnos, sugerencia, entrar, salir, error } =
    useInterview(activeId);

  return (
    <>
      <h1>Entrevista</h1>

      {!dentro && (
        <section className="card">
          <h2>Antes de empezar</h2>
          <p className="muted">
            Al entrar se abren las dos capturas: el micrófono, para saber cuándo estás
            contestando, y el audio del sistema, para oír al entrevistador. Con altavoces en
            vez de auriculares las dos oyen lo mismo y la app no puede distinguir quién habla.
          </p>

          {projects.state.status === 'ready' && projects.state.value.length > 0 && (
            <div className="row">
              <select
                value={activeId ?? ''}
                onChange={(event) => {
                  setProjectId(Number(event.target.value));
                }}
              >
                {projects.state.value.map((project) => (
                  <option key={project.id} value={project.id}>
                    {project.name}
                  </option>
                ))}
              </select>
              <button type="button" className="btn" onClick={entrar}>
                Entrar en la entrevista
              </button>
            </div>
          )}

          {projects.state.status === 'ready' && projects.state.value.length === 0 && (
            <p className="muted">Crea un proyecto en Proyectos antes de empezar.</p>
          )}

          {error !== null && <p className="error">{error}</p>}
        </section>
      )}

      {dentro && (
        <>
          <section className="card">
            <div className="slide__head">
              <span className={view.state === 'asking' ? 'vad__speaking' : 'muted small'}>
                {ESTADOS[view.state]}
              </span>
              <button type="button" className="btn btn--ghost" onClick={salir}>
                Salir
              </button>
            </div>

            {pregunta === null ? (
              <p className="muted">
                Esperando la primera pregunta. Lo que se dijera antes de entrar no cuenta.
              </p>
            ) : (
              <h2 className="slide__question">{pregunta}</h2>
            )}

            {view.skipped > 0 && (
              <p className="muted small">
                {view.skipped} turno{view.skipped === 1 ? '' : 's'} sin sugerencia por no
                parecer una pregunta. Si esto sube rápido, avísame: el filtro se estaría
                comiendo preguntas de verdad.
              </p>
            )}

            {error !== null && <p className="error">{error}</p>}
          </section>

          {pregunta !== null && (
            <section className="card">
              {sugerencia === null && <p className="muted">Preparando…</p>}

              {sugerencia?.sinMaterial !== undefined && (
                <div className="notice notice--warn">
                  <strong>No hay material tuyo para contestar esto.</strong>
                  <p className="muted">{sugerencia.sinMaterial}</p>
                  <p className="muted small">
                    No es un fallo: es que esa pregunta no está entrenada. Contéstala tú y
                    déjala luego en Entrenamiento.
                  </p>
                </div>
              )}

              {sugerencia !== null && sugerencia.texto !== '' && (
                <p className="answer">{sugerencia.texto}</p>
              )}

              {sugerencia !== null && sugerencia.keyPoints.length > 0 && (
                <>
                  <h3 className="small">Puntos que no se te pueden olvidar</h3>
                  <ul className="reasons">
                    {sugerencia.keyPoints.map((point) => (
                      <li key={point}>{point}</li>
                    ))}
                  </ul>
                </>
              )}

              {sugerencia !== null && sugerencia.followUps.length > 0 && (
                <>
                  <h3 className="small">Por dónde puede seguir</h3>
                  <ul className="reasons">
                    {sugerencia.followUps.map((point) => (
                      <li key={point}>{point}</li>
                    ))}
                  </ul>
                </>
              )}

              {sugerencia?.fallo !== undefined && <p className="error">{sugerencia.fallo}</p>}

              {sugerencia?.elapsedMs !== undefined && (
                <p className="muted small">
                  {(sugerencia.elapsedMs / 1000).toFixed(1)} s ·{' '}
                  {sugerencia.citas ?? 0} cita{sugerencia.citas === 1 ? '' : 's'} verificada
                  {sugerencia.citas === 1 ? '' : 's'} ·{' '}
                  {sugerencia.sentTo === '' || sugerencia.sentTo === undefined
                    ? 'sin salir del equipo'
                    : `enviado a ${sugerencia.sentTo}`}
                </p>
              )}
            </section>
          )}

          <section className="card">
            <h2>Lo que se ha dicho</h2>
            <TranscriptPanel transcript={turnos} />
          </section>
        </>
      )}
    </>
  );
}
