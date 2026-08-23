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
//!
//! Lo que **todavia no esta**: engancharla al audio de verdad y a `llm::answering`. Va
//! aparte a proposito. Una maquina de estados que se estrena ya conectada a un microfono, un
//! modelo y una pantalla no se depura, se adivina — y en este proyecto los cinco fallos que
//! se han encontrado usando la aplicacion y no con un test han salido todos de ahi.

pub mod machine;
pub mod trigger;
