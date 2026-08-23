import { describe, expect, it } from 'vitest';
import { SEGUNDOS_PARA_AVANZAR, siguienteFase } from '@/components/training/flujo';
import type { Accion, Contexto, Fase } from '@/components/training/flujo';
import type { AnswerReview } from '@/ipc/types';

const LIBRE: Contexto = { ocupado: false, hayTexto: true };
const HABLANDO: Contexto = { ocupado: true, hayTexto: true };
const VACIO: Contexto = { ocupado: false, hayTexto: false };

const REVIEW: AnswerReview = {
  suspicious: true,
  reasons: [{ kind: 'tooShort', words: 4 }],
  words: 4,
};

/** Encadena acciones desde una fase, para leer los casos como una secuencia. */
function tras(inicial: Fase, acciones: readonly (readonly [Accion, Contexto])[]): Fase {
  return acciones.reduce((fase, [accion, ctx]) => siguienteFase(fase, accion, ctx), inicial);
}

const tic = (ctx: Contexto) => [{ tipo: 'tic' } as const, ctx] as const;

describe('la cuenta para avanzar', () => {
  it('se arma sola cuando hay texto y no queda nada en marcha', () => {
    expect(tras({ tipo: 'respondiendo' }, [tic(LIBRE)])).toEqual({
      tipo: 'avanzando',
      quedan: SEGUNDOS_PARA_AVANZAR,
    });
  });

  it('no se arma si no hay nada escrito', () => {
    expect(tras({ tipo: 'respondiendo' }, [tic(VACIO)])).toEqual({ tipo: 'respondiendo' });
  });

  it('llega a comprobar tras los segundos que dice la constante', () => {
    const pasos = Array.from({ length: SEGUNDOS_PARA_AVANZAR + 1 }, () => tic(LIBRE));
    expect(tras({ tipo: 'respondiendo' }, pasos)).toEqual({ tipo: 'comprobando' });
  });

  /**
   * El fallo del 22-08. Cancelar al hablar era un efecto disparado por el **cambio** de la
   * señal, y con 3,5 s de retraso en la transcripción el texto de una frase llega cuando ya
   * has empezado la siguiente: la señal llevaba rato en `true`, no cambiaba, el efecto no
   * corría y la cuenta llegaba al final. Cortaba a mitad de respuesta.
   */
  it('se para mientras se sigue hablando, aunque se llevara rato hablando', () => {
    const fase = tras({ tipo: 'avanzando', quedan: 1 }, [tic(HABLANDO), tic(HABLANDO)]);
    expect(fase).toEqual({ tipo: 'respondiendo' });
  });

  /** La otra mitad del mismo fallo: queda audio sin transcribir y todavía falta texto. */
  it('se para mientras whisper no ha devuelto lo que queda', () => {
    const conCola: Contexto = { ocupado: true, hayTexto: true };
    expect(tras({ tipo: 'avanzando', quedan: 1 }, [tic(conCola)])).toEqual({
      tipo: 'respondiendo',
    });
  });

  /**
   * Y vuelve sola en cuanto se queda todo quieto. Sin esto, una tos abre turno, la cuenta se
   * para, la duración mínima descarta el turno y **no llega ningún texto**: la pantalla se
   * quedaba esperando para siempre con la respuesta entera escrita.
   */
  it('vuelve a armarse cuando se queda todo quieto', () => {
    const fase = tras({ tipo: 'avanzando', quedan: 2 }, [tic(HABLANDO), tic(LIBRE)]);
    expect(fase).toEqual({ tipo: 'avanzando', quedan: SEGUNDOS_PARA_AVANZAR });
  });

  it('si se borra el texto a media cuenta, se queda quieta', () => {
    expect(tras({ tipo: 'avanzando', quedan: 2 }, [tic(VACIO)])).toEqual({ tipo: 'quieto' });
  });
});

describe('escribir a mano', () => {
  it('desarma el avance', () => {
    expect(tras({ tipo: 'avanzando', quedan: 2 }, [[{ tipo: 'aMano' }, LIBRE]])).toEqual({
      tipo: 'quieto',
    });
  });

  /** Y el rearme automático no puede pisarlo: sería deshacer el botón al segundo. */
  it('y el rearme automático no lo deshace', () => {
    const fase = tras({ tipo: 'avanzando', quedan: 2 }, [
      [{ tipo: 'aMano' }, LIBRE],
      tic(LIBRE),
      tic(LIBRE),
    ]);
    expect(fase).toEqual({ tipo: 'quieto' });
  });

  it('pero volver a hablar sí rearma', () => {
    const fase = tras({ tipo: 'quieto' }, [[{ tipo: 'dictado' }, LIBRE]]);
    expect(fase).toEqual({ tipo: 'avanzando', quedan: SEGUNDOS_PARA_AVANZAR });
  });
});

describe('un trozo de transcripción que llega tarde', () => {
  /**
   * El peor de los cinco: relanzaba la cuenta desde `revisando` y guardaba sola la respuesta
   * que se acababa de marcar como sospechosa, saltándose la confirmación entera.
   */
  it('no relanza la cuenta mientras se está decidiendo qué hacer con la respuesta', () => {
    const fase: Fase = { tipo: 'revisando', review: REVIEW };
    expect(tras(fase, [[{ tipo: 'dictado' }, LIBRE]])).toEqual(fase);
  });

  /** Y desde `guardando` reactivaba el botón con un guardado en vuelo: dos guardados. */
  it('no toca nada mientras se está guardando', () => {
    expect(tras({ tipo: 'guardando' }, [[{ tipo: 'dictado' }, LIBRE]])).toEqual({
      tipo: 'guardando',
    });
  });

  it('tampoco mientras se comprueba', () => {
    expect(tras({ tipo: 'comprobando' }, [[{ tipo: 'dictado' }, LIBRE]])).toEqual({
      tipo: 'comprobando',
    });
  });
});

describe('guardar', () => {
  it('pasa siempre por la comprobación, también desde el botón', () => {
    expect(tras({ tipo: 'quieto' }, [[{ tipo: 'guardar' }, LIBRE]])).toEqual({
      tipo: 'comprobando',
    });
  });

  it('sin nada escrito no comprueba nada', () => {
    expect(tras({ tipo: 'respondiendo' }, [[{ tipo: 'guardar' }, VACIO]])).toEqual({
      tipo: 'quieto',
    });
  });

  it('una respuesta sospechosa para y pide decidir', () => {
    const fase = tras({ tipo: 'comprobando' }, [[{ tipo: 'sospechosa', review: REVIEW }, LIBRE]]);
    expect(fase).toEqual({ tipo: 'revisando', review: REVIEW });
  });

  it('una limpia se guarda sin preguntar', () => {
    expect(tras({ tipo: 'comprobando' }, [[{ tipo: 'limpia' }, LIBRE]])).toEqual({
      tipo: 'guardando',
    });
  });

  /**
   * Y un fallo deja quieto, no armado. Con el rearme puesto, volver a la fase armada
   * reintentaría guardar cada dos segundos contra un error que no se arregla solo.
   */
  it('un fallo no se reintenta solo', () => {
    const fase = tras({ tipo: 'guardando' }, [[{ tipo: 'fallo' }, LIBRE], tic(LIBRE), tic(LIBRE)]);
    expect(fase).toEqual({ tipo: 'quieto' });
  });
});

describe('cambiar de pregunta', () => {
  it('deja la pantalla lista y sin cuenta corriendo', () => {
    expect(tras({ tipo: 'guardando' }, [[{ tipo: 'reiniciar' }, VACIO]])).toEqual({
      tipo: 'respondiendo',
    });
  });

  it('y terminar manda al final desde donde sea', () => {
    expect(tras({ tipo: 'avanzando', quedan: 1 }, [[{ tipo: 'terminar' }, LIBRE]])).toEqual({
      tipo: 'fin',
    });
  });

  it('en el final ya no corre nada', () => {
    expect(tras({ tipo: 'fin' }, [tic(LIBRE), [{ tipo: 'dictado' }, LIBRE]])).toEqual({
      tipo: 'fin',
    });
  });
});
