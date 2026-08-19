import type { ProviderKind } from '@/ipc/types';
import { PROVIDER_LABELS, PROVIDER_NOTES } from '@/components/llm/providers';

interface Props {
  readonly value: ProviderKind;
  /** Los que ofrece el backend. Mientras no se sepan, al menos el que ya está elegido. */
  readonly available: readonly ProviderKind[];
  readonly onChange: (kind: ProviderKind) => void;
}

export function ProviderPicker({ value, available, onChange }: Props) {
  return (
    <>
      <label>
        Proveedor
        <select
          value={value}
          onChange={(event) => {
            onChange(event.target.value as ProviderKind);
          }}
        >
          {available.map((kind) => (
            <option key={kind} value={kind}>
              {PROVIDER_LABELS[kind]}
            </option>
          ))}
        </select>
      </label>

      {/* §15: si los datos salen del equipo, hay que decirlo donde se elige, no en una
          pantalla de ayuda que nadie abre. */}
      <p className={value === 'open_ai' ? 'notice notice--cloud' : 'muted small'}>
        {value === 'open_ai' && <strong>Cloud processing enabled. </strong>}
        {PROVIDER_NOTES[value]}
      </p>
    </>
  );
}
