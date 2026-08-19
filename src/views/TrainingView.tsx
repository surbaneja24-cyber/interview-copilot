import { useCallback, useState } from 'react';
import { savePreparedAnswer, trainingQuestions } from '@/ipc/commands';
import { describeError, useAsync } from '@/hooks/useAsync';
import { AnswerBox } from '@/components/training/AnswerBox';
import type { QuestionKind, TrainingStatus } from '@/ipc/types';

const KIND_LABELS: Record<QuestionKind, string> = {
  behavioral: 'Comportamental',
  motivation: 'Motivación',
  experience: 'Experiencia',
  situational: 'Situacional',
  selfAssessment: 'Sobre ti',
  logistics: 'Condiciones',
};

/**
 * Entrenamiento previo: el candidato contesta antes las preguntas que le harán después.
 *
 * Es lo que sostiene §6. La aplicación no puede inventar experiencia, así que durante la
 * entrevista solo sabrá componer con lo que haya; esta pantalla es donde se pone ese "lo que
 * haya", con las palabras del candidato. Nada de esto cuelga de una oferta concreta: sirve
 * para esta entrevista y para todas las siguientes.
 */
export function TrainingView() {
  const questions = useAsync(trainingQuestions);
  const [open, setOpen] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const onSave = useCallback(
    (question: TrainingStatus, answer: string) => {
      setError(null);
      setMessage(null);

      savePreparedAnswer(question.text, answer, question.kind)
        .then((report) => {
          setMessage(
            `Guardada e indexada en ${String(report.chunks)} fragmento${
              report.chunks === 1 ? '' : 's'
            }.`,
          );
          setOpen(null);
          questions.reload();
        })
        .catch((cause: unknown) => {
          setError(describeError(cause));
        });
    },
    [questions],
  );

  const ready = questions.state.status === 'ready';
  const answered = ready
    ? questions.state.value.filter((question) => question.answer !== null).length
    : 0;
  const total = ready ? questions.state.value.length : 0;

  return (
    <>
      <h1>Entrenamiento</h1>

      <section className="card">
        <h2>Por qué esto importa</h2>
        <p className="muted">
          La aplicación no puede inventarte experiencia, y no debe. Durante la entrevista solo
          sabrá componer con lo que le hayas contado antes. Cada respuesta que escribas o
          dictes aquí se guarda contigo, no con una oferta: vale para esta entrevista y para
          las siguientes, y cuantas más haya, mejor y más rápido responderá.
        </p>
        {total > 0 && (
          <p className="muted small">
            {answered} de {total} contestadas.
          </p>
        )}
        {message !== null && <p className="muted small">{message}</p>}
        {error !== null && <p className="error">{error}</p>}
      </section>

      {questions.state.status === 'loading' && <p className="muted">Cargando…</p>}
      {questions.state.status === 'error' && <p className="error">{questions.state.message}</p>}

      {questions.state.status === 'ready' &&
        questions.state.value.map((question) => (
          <section className="card" key={question.id}>
            <div className="model__head">
              <h3>{question.text}</h3>
              <span className={`status ${question.answer !== null ? 'status--on' : ''}`}>
                <i />
                {question.answer !== null ? 'contestada' : KIND_LABELS[question.kind]}
              </span>
            </div>

            <p className="muted small">{question.hint}</p>

            {open === question.id ? (
              <AnswerBox
                onCancel={() => {
                  setOpen(null);
                }}
                onSave={(answer) => {
                  onSave(question, answer);
                }}
              />
            ) : (
              <div className="model__actions">
                <button
                  type="button"
                  className="btn btn--ghost"
                  onClick={() => {
                    setOpen(question.id);
                    setMessage(null);
                  }}
                >
                  {question.answer !== null ? 'Volver a contestar' : 'Contestar'}
                </button>
              </div>
            )}
          </section>
        ))}
    </>
  );
}
