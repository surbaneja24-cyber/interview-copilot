import { useCallback, useEffect, useState } from 'react';
import { llmSettings, saveLlmSettings } from '@/ipc/commands';
import { describeError } from '@/hooks/useAsync';
import type { LlmSettings } from '@/ipc/types';

/**
 * Carga y persistencia de los ajustes del LLM.
 *
 * `edit` cambia solo lo que hay en pantalla y `persist` lo guarda. Están separados porque
 * un campo de texto no puede guardarse en cada tecla: se edita mientras se escribe y se
 * persiste al salir del campo. Un desplegable, en cambio, persiste directamente.
 *
 * `settings` es `null` mientras se carga; quien lo use debe tratar ese caso antes de
 * dibujar nada, porque no existen unos ajustes por defecto en el frontend: los de verdad
 * los decide el backend.
 */
export function useLlmSettings() {
  const [settings, setSettings] = useState<LlmSettings | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    llmSettings()
      .then(setSettings)
      .catch((cause: unknown) => {
        setError(describeError(cause));
      });
  }, []);

  const edit = useCallback((next: LlmSettings) => {
    setSettings(next);
    setMessage(null);
  }, []);

  const persist = useCallback((next: LlmSettings) => {
    setSettings(next);
    setError(null);
    saveLlmSettings(next)
      .then(() => {
        setMessage('Guardado.');
      })
      .catch((cause: unknown) => {
        setError(describeError(cause));
      });
  }, []);

  return { settings, message, error, edit, persist } as const;
}
