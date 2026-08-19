import { useCallback, useEffect, useRef, useState } from 'react';
import { captureStatus, stopCapture } from '@/ipc/commands';
import { describeError } from '@/hooks/useAsync';
import { SourcePanel } from '@/components/audio/SourcePanel';
import type { CaptureSnapshot } from '@/ipc/types';

/** Cada cuánto se pregunta el nivel. Una sola consulta trae las dos fuentes. */
const POLL_MS = 100;

/**
 * Audio de la entrevista (§11): el micrófono y lo que suena en el equipo.
 *
 * El medidor no es un adorno. Es la única forma de saber, antes de la entrevista y no
 * durante, si lo que está abierto es de verdad lo que oye: un selector sin medidor deja al
 * usuario capturando el micrófono de una webcam apagada y enterándose tarde.
 */
export function AudioCard() {
  const [snapshot, setSnapshot] = useState<CaptureSnapshot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const timer = useRef<number | null>(null);

  const poll = useCallback(() => {
    captureStatus()
      .then((next) => {
        setSnapshot(next);
        setError(null);
      })
      .catch((cause: unknown) => {
        setError(describeError(cause));
      });
  }, []);

  useEffect(() => {
    poll();
    timer.current = window.setInterval(poll, POLL_MS);

    return () => {
      if (timer.current !== null) window.clearInterval(timer.current);
      // Salir de la pantalla suelta el audio. Quedarse escuchando porque el usuario cambió
      // de pestaña sería escuchar sin decirlo.
      void stopCapture('mic');
      void stopCapture('system');
    };
  }, [poll]);

  const indicator = snapshot?.indicator ?? 'OFF';

  return (
    <section className="card">
      <div className="model__head">
        <h2>Audio</h2>
        <span className={`status ${indicator === 'OFF' ? '' : 'status--on'}`}>
          <i />
          {indicator}
        </span>
      </div>

      <p className="muted">
        Dos fuentes separadas a propósito: por el micrófono hablas tú y por el audio del
        sistema habla el entrevistador. Es así como la app distingue quién dice qué, sin
        tener que reconocer voces. Con altavoces en vez de auriculares tu voz vuelve por el
        audio del sistema y esa separación deja de funcionar.
      </p>

      <SourcePanel
        source="mic"
        title="Micrófono"
        explanation="Tu voz. Habla y la barra tiene que moverse."
        silenceHint="El dispositivo abrió pero no entrega ninguna muestra. Suele ser el micrófono silenciado por hardware o el permiso de micrófono de Windows."
        status={snapshot?.mic ?? null}
        onChanged={poll}
      />

      <SourcePanel
        source="system"
        title="Audio del sistema"
        explanation="Lo que suena en tu equipo, que en una entrevista es la voz del entrevistador. Se graba abriendo la salida —los altavoces o los auriculares— en modo captura; por eso aquí se eligen salidas y no entradas."
        silenceHint="La salida abrió pero no llega nada. Si acabas de cambiar de dispositivo de reproducción, vuelve a arrancar la captura sobre el nuevo."
        status={snapshot?.system ?? null}
        onChanged={poll}
      />

      {error !== null && <p className="error">{error}</p>}
    </section>
  );
}
