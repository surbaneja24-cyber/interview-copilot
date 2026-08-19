import { useCallback, useState } from 'react';
import { deleteAllData, hardwareReport } from '@/ipc/commands';
import { describeError, useAsync } from '@/hooks/useAsync';
import { AudioCard } from '@/components/AudioCard';
import { LlmSettingsCard } from '@/components/LlmSettingsCard';
import type { ExecutionProfile } from '@/ipc/types';

type WipeState =
  | { readonly status: 'idle' | 'working' | 'done' }
  | { readonly status: 'error'; readonly message: string };

const PROFILE_EXPLANATION: Record<ExecutionProfile, string> = {
  LOCAL: 'Todo se procesa en tu equipo. Nada sale de aquí.',
  HYBRID: 'La transcripción es local; solo el texto de la pregunta y el contexto recuperado se envían al proveedor de IA que elijas.',
  CLOUD: 'La transcripción y la generación se hacen fuera de tu equipo.',
};

export function SettingsView() {
  const { state } = useAsync(hardwareReport);
  const [wipe, setWipe] = useState<WipeState>({ status: 'idle' });

  const onWipe = useCallback(() => {
    const confirmed = window.confirm(
      'Esto borra todos tus proyectos, documentos, transcripciones y las claves de API guardadas en este equipo. No se puede deshacer. ¿Continuar?',
    );
    if (!confirmed) return;

    setWipe({ status: 'working' });
    deleteAllData()
      .then(() => {
        setWipe({ status: 'done' });
      })
      .catch((error: unknown) => {
        setWipe({ status: 'error', message: describeError(error) });
      });
  }, []);

  return (
    <>
      <h1>Ajustes</h1>

      <section className="card">
        <h2>Tu hardware</h2>

        {state.status === 'loading' && <p className="muted">Analizando el equipo…</p>}
        {state.status === 'error' && <p className="error">{state.message}</p>}

        {state.status === 'ready' && (
          <>
            <dl className="specs">
              <dt>Sistema</dt>
              <dd>{state.value.os}</dd>
              <dt>CPU</dt>
              <dd>
                {state.value.cpuBrand} — {state.value.logicalCores} hilos
              </dd>
              <dt>RAM</dt>
              <dd>
                {formatMb(state.value.totalRamMb)} totales, {formatMb(state.value.availableRamMb)}{' '}
                libres
              </dd>
              <dt>Gráfica</dt>
              <dd>
                {state.value.gpus.length === 0
                  ? 'ninguna detectada'
                  : state.value.gpus.map((gpu) => (
                      <div key={gpu.name}>
                        {gpu.name} — {formatMb(gpu.dedicatedVramMb)}{' '}
                        {gpu.discrete ? 'dedicados' : 'compartidos con la RAM'}
                      </div>
                    ))}
              </dd>
              <dt>VRAM utilizable</dt>
              <dd>
                {state.value.dedicatedVramMb === null
                  ? 'ninguna — el modelo tendría que ir en CPU'
                  : formatMb(state.value.dedicatedVramMb)}
              </dd>
            </dl>

            <h3>
              Perfil recomendado: <span className="badge">{state.value.recommendation.profile}</span>
            </h3>
            <p className="muted">{PROFILE_EXPLANATION[state.value.recommendation.profile]}</p>

            <ul className="reasons">
              {state.value.recommendation.reasons.map((reason) => (
                <li key={reason}>{reason}</li>
              ))}
            </ul>
          </>
        )}
      </section>

      <LlmSettingsCard />

      <AudioCard />

      <section className="card card--danger">
        <h2>Borrar todos los datos</h2>
        <p className="muted">
          Elimina de este equipo todos los proyectos, documentos, transcripciones y ajustes, y
          borra del almacén de credenciales de Windows las claves de API que hayas guardado.
          No se puede deshacer.
        </p>
        <button type="button" className="btn btn--danger" onClick={onWipe}>
          {wipe.status === 'working' ? 'Borrando…' : 'Borrar todos los datos'}
        </button>
        {wipe.status === 'done' && <p className="muted">Hecho. No queda nada guardado.</p>}
        {wipe.status === 'error' && <p className="error">{wipe.message}</p>}
      </section>
    </>
  );
}

function formatMb(mb: number): string {
  return mb >= 1024 ? `${(mb / 1024).toFixed(1)} GB` : `${String(mb)} MB`;
}
