import type { AudioLevel } from '@/ipc/types';

/** Extremo bajo de la barra. Por debajo de −60 dB no hay nada que enseñar. */
const FLOOR_DBFS = -60;

interface Props {
  readonly level: AudioLevel | null;
}

/**
 * Barra de nivel: la energía media como relleno y el pico retenido como marca.
 *
 * Las dos capas hacen falta. La media dice cuánto suena; el pico es lo único que enseña
 * una saturación demasiado corta para mover la media, que es justo lo que estropea una
 * transcripción sin que se note en pantalla.
 */
export function LevelMeter({ level }: Props) {
  const shown = level ?? { rmsDbfs: FLOOR_DBFS, peakDbfs: FLOOR_DBFS };

  return (
    <>
      <div className="meter">
        <div className="meter__bar" style={{ width: `${String(toPercent(shown.rmsDbfs))}%` }} />
        <div className="meter__peak" style={{ left: `${String(toPercent(shown.peakDbfs))}%` }} />
      </div>
      <div className="meter__scale">
        <span>−60 dB</span>
        <span>
          {level === null
            ? 'sin capturar'
            : `${shown.rmsDbfs.toFixed(1)} dB · pico ${shown.peakDbfs.toFixed(1)} dB`}
        </span>
        <span>0 dB</span>
      </div>
    </>
  );
}

/** De decibelios a ancho de barra. Lineal en dB, que es como se percibe el volumen. */
function toPercent(dbfs: number): number {
  if (dbfs <= FLOOR_DBFS) return 0;
  if (dbfs >= 0) return 100;
  return Math.round(((dbfs - FLOOR_DBFS) / -FLOOR_DBFS) * 100);
}
