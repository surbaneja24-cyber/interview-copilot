import { useCallback, useEffect, useState } from 'react';

export type AsyncState<T> =
  | { readonly status: 'loading' }
  | { readonly status: 'ready'; readonly value: T }
  | { readonly status: 'error'; readonly message: string };

/**
 * Los comandos de Tauri fallan de dos maneras muy distintas: un error real del backend,
 * o que no haya backend porque la app se abrió en el navegador con `npm run dev`.
 * Aquí se normalizan las dos a un mensaje legible.
 */
export function describeError(error: unknown): string {
  if (typeof error === 'string') return error;
  if (error instanceof Error) {
    if (error.message.includes('window.__TAURI_INTERNALS__')) {
      return 'Sin backend: esta ventana es el navegador. Arranca la app con "npm run tauri dev".';
    }
    return error.message;
  }
  return 'Error desconocido';
}

export function useAsync<T>(run: () => Promise<T>, deps: readonly unknown[] = []) {
  const [state, setState] = useState<AsyncState<T>>({ status: 'loading' });

  // El caller controla cuándo cambia la identidad de `run` a través de `deps`.
  const stableRun = useCallback(run, deps);

  const reload = useCallback(() => {
    let cancelled = false;
    setState({ status: 'loading' });

    stableRun()
      .then((value) => {
        if (!cancelled) setState({ status: 'ready', value });
      })
      .catch((error: unknown) => {
        if (!cancelled) setState({ status: 'error', message: describeError(error) });
      });

    return () => {
      cancelled = true;
    };
  }, [stableRun]);

  useEffect(() => reload(), [reload]);

  return { state, reload } as const;
}
