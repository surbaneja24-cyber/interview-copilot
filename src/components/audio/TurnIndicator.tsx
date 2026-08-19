import type { VadState } from '@/ipc/types';

interface Props {
  readonly vad: VadState | null;
}

/**
 * Lo que el detector de voz está viendo ahora mismo.
 *
 * Enseña la probabilidad y no solo el veredicto: sirve para ver con qué margen se está
 * decidiendo, que es lo que hará falta el día que haya que calibrar el umbral con audio de
 * entrevistas reales en vez de con el valor heredado de Silero.
 */
export function TurnIndicator({ vad }: Props) {
  if (vad === null) return null;

  const speaking = vad.turn === 'speaking';

  return (
    <div className="vad">
      <span className={speaking ? 'vad__speaking' : undefined}>
        {speaking ? 'hablando' : 'callado'}
      </span>
      <span>voz {(vad.probability * 100).toFixed(0)}%</span>
      <span>máx {(vad.maxProbability * 100).toFixed(0)}%</span>
      <span>{vad.turns} turnos</span>
      {vad.lastTurnMs !== null && <span>último {(vad.lastTurnMs / 1000).toFixed(1)} s</span>}
      {vad.dropped > 0 && (
        <span className="error">
          {vad.dropped.toLocaleString('es-ES')} muestras perdidas: el detector no vio todo el
          audio
        </span>
      )}
    </div>
  );
}
