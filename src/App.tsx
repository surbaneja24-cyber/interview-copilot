import { useState } from 'react';
import { PrepareView } from '@/views/PrepareView';
import { TrainingView } from '@/views/TrainingView';
import { ProjectsView } from '@/views/ProjectsView';
import { SettingsView } from '@/views/SettingsView';

const VIEWS = ['projects', 'prepare', 'training', 'interview', 'practice', 'settings'] as const;

type View = (typeof VIEWS)[number];

const VIEW_LABELS: Record<View, string> = {
  projects: 'Proyectos',
  prepare: 'Preparación',
  training: 'Entrenamiento',
  interview: 'Entrevista',
  practice: 'Práctica',
  settings: 'Ajustes',
};

/** Fases del roadmap que aún no existen. Se listan para no fingir que hay funcionalidad. */
const PENDING: Record<string, string> = {
  interview: 'Fase 5 — transcripción en vivo y respuestas',
  practice: 'Fase 7 — entrevista simulada y puntuación',
};

export function App() {
  const [view, setView] = useState<View>('projects');

  return (
    <div className="app">
      <nav className="app__nav" aria-label="Secciones">
        {VIEWS.map((id) => (
          <button
            key={id}
            type="button"
            className={id === view ? 'app__tab app__tab--active' : 'app__tab'}
            aria-current={id === view ? 'page' : undefined}
            onClick={() => {
              setView(id);
            }}
          >
            {VIEW_LABELS[id]}
          </button>
        ))}
      </nav>

      <main className="app__main">
        {view === 'projects' && <ProjectsView />}
        {view === 'prepare' && <PrepareView />}
        {view === 'training' && <TrainingView />}
        {view === 'settings' && <SettingsView />}
        {PENDING[view] !== undefined && (
          <>
            <h1>{VIEW_LABELS[view]}</h1>
            <p className="muted">Pendiente: {PENDING[view]}</p>
          </>
        )}
      </main>
    </div>
  );
}
