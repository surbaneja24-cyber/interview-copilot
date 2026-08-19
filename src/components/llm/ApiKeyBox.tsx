import { useCallback, useEffect, useState } from 'react';
import { apiKeyPresent, clearApiKey, setApiKey } from '@/ipc/commands';
import { describeError } from '@/hooks/useAsync';
import type { ProviderKind } from '@/ipc/types';

interface Props {
  readonly provider: ProviderKind;
}

/**
 * Alta y baja de la clave de API.
 *
 * Aquí solo se puede poner una, borrarla o preguntar si hay alguna: no existe ningún
 * comando que devuelva una clave al frontend, y por eso §31 se cumple por construcción y
 * no por acordarse de no enseñarla (ver `ARCHITECTURE.md` §2).
 */
export function ApiKeyBox({ provider }: Props) {
  const [present, setPresent] = useState(false);
  const [draft, setDraft] = useState('');
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setDraft('');
    setMessage(null);
    apiKeyPresent(provider)
      .then(setPresent)
      .catch(() => {
        setPresent(false);
      });
  }, [provider]);

  const onSave = useCallback(() => {
    setError(null);
    setApiKey(provider, draft)
      .then(() => {
        setDraft('');
        setPresent(true);
        setMessage('Clave guardada en el almacén de credenciales de Windows.');
      })
      .catch((cause: unknown) => {
        setError(describeError(cause));
      });
  }, [provider, draft]);

  const onClear = useCallback(() => {
    setError(null);
    clearApiKey(provider)
      .then(() => {
        setPresent(false);
        setMessage('Clave borrada.');
      })
      .catch((cause: unknown) => {
        setError(describeError(cause));
      });
  }, [provider]);

  return (
    <>
      <h3>Clave de API</h3>
      <p className="muted small">
        Se guarda en el Administrador de credenciales de Windows, no en la base de datos de
        la aplicación. No hay forma de volver a mostrarla desde aquí: solo sustituirla o
        borrarla.
      </p>
      <div className="row">
        <input
          className="grow"
          type="password"
          autoComplete="off"
          placeholder={present ? '•••••••• (hay una guardada)' : 'sk-…'}
          value={draft}
          onChange={(event) => {
            setDraft(event.target.value);
          }}
        />
        <button type="button" className="btn" disabled={draft === ''} onClick={onSave}>
          Guardar
        </button>
        {present && (
          <button type="button" className="btn btn--ghost" onClick={onClear}>
            Borrar
          </button>
        )}
      </div>

      {message !== null && <p className="muted small">{message}</p>}
      {error !== null && <p className="error">{error}</p>}
    </>
  );
}
