import { useCallback, useState, type SyntheticEvent } from 'react';
import { createProject, deleteProject, listProjects } from '@/ipc/commands';
import { describeError, useAsync } from '@/hooks/useAsync';

const EMPTY_FORM = { name: '', company: '', role: '' };

export function ProjectsView() {
  const { state, reload } = useAsync(listProjects);
  const [form, setForm] = useState(EMPTY_FORM);
  const [formError, setFormError] = useState<string | null>(null);

  const onSubmit = useCallback(
    (event: SyntheticEvent<HTMLFormElement>) => {
      event.preventDefault();
      setFormError(null);

      createProject(form)
        .then(() => {
          setForm(EMPTY_FORM);
          reload();
        })
        .catch((error: unknown) => {
          setFormError(describeError(error));
        });
    },
    [form, reload],
  );

  const onDelete = useCallback(
    (id: number) => {
      deleteProject(id)
        .then(() => {
          reload();
        })
        .catch((error: unknown) => {
          setFormError(describeError(error));
        });
    },
    [reload],
  );

  return (
    <>
      <h1>Proyectos</h1>

      <section className="card">
        <h2>Nuevo proyecto</h2>
        <form className="form" onSubmit={onSubmit}>
          <label>
            Nombre
            <input
              value={form.name}
              required
              placeholder="Google — Software Engineer"
              onChange={(event) => {
                setForm((prev) => ({ ...prev, name: event.target.value }));
              }}
            />
          </label>
          <label>
            Empresa
            <input
              value={form.company}
              placeholder="Google"
              onChange={(event) => {
                setForm((prev) => ({ ...prev, company: event.target.value }));
              }}
            />
          </label>
          <label>
            Puesto
            <input
              value={form.role}
              placeholder="Software Engineer"
              onChange={(event) => {
                setForm((prev) => ({ ...prev, role: event.target.value }));
              }}
            />
          </label>
          <button type="submit" className="btn">
            Crear
          </button>
        </form>
        {formError !== null && <p className="error">{formError}</p>}
      </section>

      <section className="card">
        <h2>Tus proyectos</h2>
        {state.status === 'loading' && <p className="muted">Cargando…</p>}
        {state.status === 'error' && <p className="error">{state.message}</p>}
        {state.status === 'ready' &&
          (state.value.length === 0 ? (
            <p className="muted">Todavía no hay ninguno.</p>
          ) : (
            <ul className="projects">
              {state.value.map((project) => (
                <li key={project.id}>
                  <div>
                    <strong>{project.name}</strong>
                    <span className="muted">
                      {[project.company, project.role].filter(Boolean).join(' · ')}
                    </span>
                  </div>
                  <button
                    type="button"
                    className="btn btn--ghost"
                    onClick={() => {
                      onDelete(project.id);
                    }}
                  >
                    Borrar
                  </button>
                </li>
              ))}
            </ul>
          ))}
      </section>
    </>
  );
}
