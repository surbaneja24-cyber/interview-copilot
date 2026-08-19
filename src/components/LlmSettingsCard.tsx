import { useCallback } from 'react';
import { llmProviders } from '@/ipc/commands';
import { useAsync } from '@/hooks/useAsync';
import { useLlmSettings } from '@/hooks/useLlmSettings';
import { ApiKeyBox } from '@/components/llm/ApiKeyBox';
import { ModelPicker } from '@/components/llm/ModelPicker';
import { ProviderPicker } from '@/components/llm/ProviderPicker';
import { defaultBaseUrl, defaultModel, hasServer, needsApiKey } from '@/components/llm/providers';
import type { ProviderKind } from '@/ipc/types';

/**
 * La tarjeta de ajustes del LLM: elegir proveedor, servidor y modelo, y guardar la clave.
 *
 * Aquí solo se compone y se decide qué se enseña con cada proveedor. Lo demás vive
 * separado porque son tres cosas que fallan por su cuenta —cargar los ajustes, consultar
 * los modelos al servidor y hablar con el almacén de credenciales— y cada una tiene que
 * poder decirlo en su sitio en vez de compartir un único hueco de mensaje.
 */
export function LlmSettingsCard() {
  const providers = useAsync(llmProviders);
  const { settings, message, error, edit, persist } = useLlmSettings();

  const onProviderChange = useCallback(
    (kind: ProviderKind) => {
      if (settings === null) return;
      // Cambiar de proveedor cambia URL y modelo por defecto: conservarlos apuntaría el
      // proveedor nuevo a un servidor que no es el suyo.
      persist({
        ...settings,
        kind,
        baseUrl: defaultBaseUrl(kind),
        model: defaultModel(kind),
      });
    },
    [persist, settings],
  );

  if (settings === null) {
    return (
      <section className="card">
        <h2>Modelo de lenguaje</h2>
        {error === null ? <p className="muted">Cargando…</p> : <p className="error">{error}</p>}
      </section>
    );
  }

  return (
    <section className="card">
      <h2>Modelo de lenguaje</h2>

      <div className="form">
        <ProviderPicker
          value={settings.kind}
          available={providers.state.status === 'ready' ? providers.state.value : [settings.kind]}
          onChange={onProviderChange}
        />

        {hasServer(settings.kind) && (
          <ModelPicker settings={settings} onEdit={edit} onPersist={persist} />
        )}

        {needsApiKey(settings.kind) && <ApiKeyBox provider={settings.kind} />}
      </div>

      {message !== null && <p className="muted small">{message}</p>}
      {error !== null && <p className="error">{error}</p>}
    </section>
  );
}
