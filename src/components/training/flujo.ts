import type { AnswerReview } from '@/ipc/types';

/**
 * La máquina de estados del modo diapositiva, sin React dentro.
 *
 * Existe por una lista de fallos, no por gusto arquitectónico. Los cinco que ha tenido esta
 * pantalla se encontraron **usando la aplicación o releyéndola**, ninguno con un test, y los
 * cinco eran de lo mismo: no de qué se calcula, sino de *cuándo* corría un efecto.
 *
 * 1. Un `setInterval` huérfano escribía en la pregunta siguiente (21-08).
 * 2. El contador de turnos descartados salía en la pantalla equivocada (22-08).
 * 3. Cancelar la cuenta al hablar era un efecto por *cambio* de señal, y con 3,5 s de
 *    retraso en la transcripción la señal llevaba rato en `true` y no cambiaba: cortaba a
 *    mitad de respuesta (22-08).
 * 4. Un trozo de transcripción tardío relanzaba la cuenta desde `revisando`, saltándose la
 *    confirmación que se acababa de poner (22-08).
 * 5. La cuenta solo se rearmaba al llegar texto, así que una tos la dejaba parada para
 *    siempre (22-08).
 *
 * Escrita como una función pura, los cinco son casos de prueba de tres líneas. La regla que
 * sale de ahí y que gobierna todo este fichero: **las decisiones se toman con el valor de
 * ahora, no con el instante en que algo cambió.** Por eso `tic` recibe el contexto entero en
 * cada paso en vez de reaccionar a transiciones.
 *
 * Lo que queda en el componente es obedecer: pedir la revisión, guardar, y llamar a `tic`
 * una vez por segundo.
 */

/** Segundos de silencio, ya con el texto en pantalla, antes de pasar a la siguiente. */
export const SEGUNDOS_PARA_AVANZAR = 2;

export type Fase =
  /** Esperando, con el avance automático armado. */
  | { readonly tipo: 'respondiendo' }
  /** Esperando, desarmado a mano: el usuario escribió o pidió quedarse. */
  | { readonly tipo: 'quieto' }
  | { readonly tipo: 'avanzando'; readonly quedan: number }
  /** Se está mirando la respuesta antes de guardarla. */
  | { readonly tipo: 'comprobando' }
  /** Se parece a las que salieron mal: hay que decidir. */
  | { readonly tipo: 'revisando'; readonly review: AnswerReview }
  | { readonly tipo: 'guardando' }
  | { readonly tipo: 'fin' };

export type Accion =
  /** Ha llegado texto del micrófono. */
  | { readonly tipo: 'dictado' }
  /** El usuario ha escrito a mano, o ha pedido quedarse. */
  | { readonly tipo: 'aMano' }
  /** Ha pasado un segundo. Es la única fuente de tiempo. */
  | { readonly tipo: 'tic' }
  /** El usuario ha pulsado guardar. */
  | { readonly tipo: 'guardar' }
  | { readonly tipo: 'sospechosa'; readonly review: AnswerReview }
  | { readonly tipo: 'limpia' }
  /** Falló guardar, o falló la comprobación. */
  | { readonly tipo: 'fallo' }
  /** Borrar y volver a empezar esta misma respuesta. */
  | { readonly tipo: 'repetir' }
  /** Pregunta nueva. */
  | { readonly tipo: 'reiniciar' }
  | { readonly tipo: 'terminar' };

export interface Contexto {
  /**
   * Hay algo en marcha: voz que el VAD oye, o audio que whisper no ha devuelto.
   *
   * Las dos dicen lo mismo — que la respuesta en pantalla no está completa — y ninguna lleva
   * número dentro. La segunda es la que no se ve: entre callarse y ver el texto hay 3,7 s en
   * los que no se oye nada y todavía falta texto por llegar.
   */
  readonly ocupado: boolean;
  readonly hayTexto: boolean;
}

/** Fases desde las que el dictado puede rearmar la cuenta. */
function esperando(fase: Fase): boolean {
  return fase.tipo === 'respondiendo' || fase.tipo === 'quieto' || fase.tipo === 'avanzando';
}

export function siguienteFase(fase: Fase, accion: Accion, ctx: Contexto): Fase {
  switch (accion.tipo) {
    case 'terminar':
      return { tipo: 'fin' };

    case 'reiniciar':
    case 'repetir':
      return { tipo: 'respondiendo' };

    case 'dictado':
      // Solo desde las fases que esperan. Desde `revisando` relanzaría la cuenta y guardaría
      // sola la respuesta que se acaba de marcar; desde `guardando` reactivaría el botón con
      // un guardado en vuelo. Con 3,5 s de retraso, un trozo tardío es el caso normal.
      return esperando(fase) ? { tipo: 'avanzando', quedan: SEGUNDOS_PARA_AVANZAR } : fase;

    case 'aMano':
      // Escribir desarma hasta que se vuelva a hablar. Sin una fase propia, el rearme
      // automático pisaría el botón de "Quedarme" un segundo después de pulsarlo.
      return esperando(fase) ? { tipo: 'quieto' } : fase;

    case 'guardar':
      if (!ctx.hayTexto) return { tipo: 'quieto' };
      return { tipo: 'comprobando' };

    case 'sospechosa':
      return fase.tipo === 'comprobando' ? { tipo: 'revisando', review: accion.review } : fase;

    case 'limpia':
      return fase.tipo === 'comprobando' ? { tipo: 'guardando' } : fase;

    case 'fallo':
      // A `quieto` y no a `respondiendo`: con el rearme automático puesto, volver a la fase
      // armada reintentaría cada dos segundos contra un error que no se arregla solo.
      return { tipo: 'quieto' };

    case 'tic': {
      if (fase.tipo !== 'respondiendo' && fase.tipo !== 'avanzando') return fase;

      // El valor de **ahora**, no el instante en que cambió. Es el fallo 3 de la lista de
      // arriba, y el motivo de que esto sea una función y no un efecto.
      if (ctx.ocupado) return { tipo: 'respondiendo' };

      if (fase.tipo === 'respondiendo') {
        // Rearme. Sin esto, una tos abre turno, para la cuenta, el turno se descarta por
        // corto y no llega ningún texto: la pantalla se queda esperando para siempre con la
        // respuesta entera escrita.
        return ctx.hayTexto ? { tipo: 'avanzando', quedan: SEGUNDOS_PARA_AVANZAR } : fase;
      }

      if (!ctx.hayTexto) return { tipo: 'quieto' };
      return fase.quedan > 1 ? { tipo: 'avanzando', quedan: fase.quedan - 1 } : { tipo: 'comprobando' };
    }
  }
}
