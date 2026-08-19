import { useCallback, useState } from 'react';
import { useDictation } from '@/hooks/useDictation';

interface Props {
  readonly onSave: (answer: string) => void;
  readonly onCancel: () => void;
}

/**
 * Responder a una pregunta suelta, escribiendo o hablando.
 *
 * Es la forma de contestar desde la lista, cuando se quiere corregir una respuesta
 * concreta. Para contestar muchas seguidas está el modo diapositiva, que quita los clics
 * de en medio; el dictado en si es el mismo en los dos (`useDictation`).
 */
export function AnswerBox({ onSave, onCancel }: Props) {
  const [text, setText] = useState('');

  const recibir = useCallback((trozo: string) => {
    setText((previo) => [previo, trozo].filter(Boolean).join(' '));
  }, []);

  const dictado = useDictation(recibir);

  return (
    <div className="form">
      <textarea
        className="answer-box"
        rows={6}
        value={text}
        placeholder="Escribe tu respuesta, o dale a Dictar y cuéntala en voz alta."
        onChange={(event) => {
          setText(event.target.value);
        }}
      />

      {dictado.dictating && (
        <>
          <p className="muted small">
            Escuchando. El texto aparece cuando terminas de hablar, no mientras hablas: hacen
            falta 700 ms de silencio para dar la frase por acabada.
          </p>
          <p className="muted small">{dictado.status}</p>
        </>
      )}

      <div className="model__actions">
        <button
          type="button"
          className="btn"
          disabled={text.trim() === ''}
          onClick={() => {
            dictado.stop();
            onSave(text.trim());
          }}
        >
          Guardar
        </button>
        <button
          type="button"
          className="btn btn--ghost"
          onClick={dictado.dictating ? dictado.stop : dictado.start}
        >
          {dictado.dictating ? 'Parar de dictar' : 'Dictar'}
        </button>
        <button
          type="button"
          className="btn btn--ghost"
          onClick={() => {
            dictado.stop();
            onCancel();
          }}
        >
          Cancelar
        </button>
      </div>

      {dictado.transcriptError !== null && <p className="error">{dictado.transcriptError}</p>}
      {dictado.error !== null && <p className="error">{dictado.error}</p>}
    </div>
  );
}
