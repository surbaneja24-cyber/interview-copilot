//! La entrevista en vivo (§7, §9 y §10).
//!
//! Las piezas de debajo estan todas montadas y medidas desde la Fase 4 —captura por dos
//! fuentes, VAD, whisper, clasificador, recuperacion con filtro—. Lo que falta es lo que las
//! conecta, y eso es esto.
//!
//! Se ha empezado por las dos partes que **pueden equivocarse sin dar error**, que son las
//! que llevan tests desde el primer dia segun la politica del roadmap:
//!
//! - `trigger` decide si un turno del entrevistador merece una pasada del pipeline. Se
//!   equivoca en silencio en las dos direcciones: de menos, deja al candidato sin ayuda; de
//!   mas, gasta una pasada. Esta medido contra los dos corpus del clasificador.
//! - `machine` decide en que punto de la entrevista estamos y cuando toca preparar algo. Es
//!   una maquina de estados sin nada de fuera dentro, igual que `TurnDetector`, y se prueba
//!   con eventos escritos a mano.
//! - `session` traduce lo que reporta el audio —entradas del transcriptor y estado del VAD—
//!   a eventos de la maquina. Es la tercera que se equivoca en silencio: repetir una entrada
//!   ya vista, perder un turno o confundir quien hablaba no dan error, dan una entrevista
//!   que contesta a destiempo.
//!
//! Lo que **todavia no esta**: pedirle la sugerencia a `llm::answering` cuando la sesion deja
//! una pregunta pendiente, y la pantalla. La pregunta se queda esperando a que alguien la
//! recoja, que es lo que permite estrenar el enganche sin estrenar tambien el modelo.

pub mod machine;
pub mod session;
pub mod trigger;
