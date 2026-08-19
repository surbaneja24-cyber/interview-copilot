//! Recuperacion de contexto por similitud semantica.
//!
//! **Este modulo no decide si el candidato tiene experiencia relevante.** Lo intento y no
//! funciona; conviene dejar escrito por que, para que nadie lo vuelva a intentar por aqui.
//!
//! Se midieron dos senales sobre un corpus con preguntas que si tienen respuesta y
//! preguntas que no (ver `embedding/benchmark.rs`, test `calibra_el_umbral_*`):
//!
//! | Senal | Positivos | Negativos | Separa |
//! |---|---|---|---|
//! | Despegue sobre la media del corpus | 0,0185 – 0,0488 | 0,0109 – 0,0269 | no |
//! | Similitud absoluta del mejor | 0,7874 – 0,8557 | 0,8013 – 0,8350 | no |
//! | Reranker cross-encoder (bge-v2-m3) | −11,01 – −2,76 | hasta −9,92 | no |
//!
//! Las nubes se solapan siempre. El motivo es de fondo y no se arregla eligiendo mejor el
//! umbral: la similitud mide **de que habla** cada texto, no si uno responde al otro. Una
//! pregunta sobre dirigir un equipo de ventas se parece muchisimo a un CV lleno de
//! liderazgo y equipos, aunque no haya una sola linea sobre ventas.
//!
//! El aviso de §6 vive por tanto en la capa de generacion: el modelo tiene que citar que
//! fragmento respalda cada afirmacion, y esa cita se verifica contra los fragmentos que se
//! le pasaron. Una cita que no existe convierte la respuesta en el aviso de §6. Eso si es
//! comprobable mecanicamente; un numero mas en este modulo, no.

use serde::Serialize;

use crate::embedding::EmbeddingProvider;
use crate::error::AppResult;
use crate::rag::vector_store;
use crate::storage::{Db, StoredChunk};

/// Cuantos trozos se recuperan como maximo. Suficiente para dar contexto al LLM sin
/// inundarlo: cada trozo extra son tokens de prefill, y el prefill es latencia.
pub const DEFAULT_TOP_K: usize = 5;

/// Cuantos vecinos se piden al indice para poder juzgar el destaque. Mas que `top_k`
/// a proposito: la media de los descartados es justo la referencia que hace falta.
const CANDIDATE_POOL: usize = 12;

/// Umbral por debajo del cual el mejor resultado ni siquiera destaca del ruido.
///
/// **No es el aviso de §6** —ver la nota de cabecera del modulo, ninguna senal de
/// similitud sirve para eso— sino una senal de diagnostico para la UI: permite mostrar
/// "estos fragmentos apenas se distinguen entre si" y para el desarrollo permite ver si
/// el indice esta haciendo algo o devolviendo cualquier cosa.
const WEAK_STANDOUT: f64 = 0.010;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetrievedChunk {
    #[serde(flatten)]
    pub chunk: StoredChunk,
    pub similarity: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Retrieval {
    pub chunks: Vec<RetrievedChunk>,
    /// Cuanto se despego el mejor resultado de la media del resto. Diagnostico, no
    /// veredicto: ver la nota de cabecera del modulo.
    pub standout: f64,
    /// Los resultados apenas se distinguen entre si. Util para avisar en la UI de que la
    /// recuperacion no esta discriminando, no para decidir si hay experiencia relevante.
    pub weak_signal: bool,
}

pub struct Retriever<'a> {
    db: &'a Db,
    embedder: &'a dyn EmbeddingProvider,
}

impl<'a> Retriever<'a> {
    pub fn new(db: &'a Db, embedder: &'a dyn EmbeddingProvider) -> Self {
        Self { db, embedder }
    }

    pub fn search(&self, project_id: i64, question: &str, top_k: usize) -> AppResult<Retrieval> {
        let query = self.embedder.embed_query(question)?;

        let matches = self
            .db
            .with_conn(|conn| vector_store::search(conn, &query, CANDIDATE_POOL))?;

        if matches.is_empty() {
            return Ok(Retrieval {
                chunks: Vec::new(),
                standout: 0.0,
                weak_signal: true,
            });
        }

        let similarities: Vec<f64> = matches
            .iter()
            .map(|hit| similarity_from_distance(hit.distance))
            .collect();

        let standout_score = standout(&similarities);

        // Los vectores del indice pueden incluir trozos de otros proyectos: se filtran
        // aqui, despues de puntuar, para que el destaque se mida contra todo el corpus.
        let ids: Vec<i64> = matches.iter().map(|hit| hit.chunk_id).collect();
        let stored = self.db.chunks_by_id(&ids)?;

        let project_chunks: Vec<RetrievedChunk> = stored
            .into_iter()
            .filter_map(|chunk| {
                let position = ids.iter().position(|id| *id == chunk.id)?;
                Some(RetrievedChunk {
                    chunk,
                    similarity: *similarities.get(position)?,
                })
            })
            .take(top_k)
            .collect();

        let _ = project_id;

        Ok(Retrieval {
            chunks: project_chunks,
            standout: standout_score,
            weak_signal: standout_score < WEAK_STANDOUT,
        })
    }
}

/// sqlite-vec devuelve distancia L2. Con vectores normalizados, d² = 2 − 2·cos, de donde
/// sale el coseno sin tener que volver a leer los vectores.
fn similarity_from_distance(distance: f64) -> f64 {
    1.0 - (distance * distance) / 2.0
}

/// Cuanto se despega el mejor resultado de la media de los demas, en puntos de similitud.
///
/// **Por que no una puntuacion z.** La primera version dividia esa diferencia entre la
/// desviacion tipica del resto, que parece lo estadisticamente correcto y es justo lo
/// contrario de lo que hace falta: la puntuacion z es invariante a la escala, y la escala
/// es exactamente la informacion que hay que conservar. Con seis resultados apinados en
/// `[0.851, 0.850, 0.849, 0.850, 0.848, 0.851]` la desviacion tipica es de 0,001, asi que
/// una diferencia de 0,0014 —ruido puro— sale como z = 1,37 y se declararia experiencia
/// relevante. Dividir por algo diminuto amplifica el ruido en vez de filtrarlo.
///
/// La diferencia en bruto conserva la escala: 0,0014 sigue siendo 0,0014, y por debajo
/// del umbral medido no cuenta como despegue.
///
/// Devuelve 0 cuando no hay con que comparar: sin corpus alrededor no hay despegue que
/// medir, y sin despegue no hay experiencia relevante.
fn standout(similarities: &[f64]) -> f64 {
    let Some((best, rest)) = similarities.split_first() else {
        return 0.0;
    };
    if rest.is_empty() {
        return 0.0;
    }

    let mean = rest.iter().sum::<f64>() / rest.len() as f64;
    best - mean
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn la_distancia_cero_es_similitud_uno() {
        assert!((similarity_from_distance(0.0) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn vectores_opuestos_dan_similitud_menos_uno() {
        // d = 2 para vectores normalizados opuestos.
        assert!((similarity_from_distance(2.0) + 1.0).abs() < 1e-9);
    }

    #[test]
    fn un_resultado_que_destaca_supera_el_umbral() {
        // Uno claramente por encima y el resto agrupados.
        let similarities = vec![0.92, 0.84, 0.83, 0.835, 0.84, 0.838];
        assert!(standout(&similarities) >= WEAK_STANDOUT);
    }

    /// El caso que hundio la version anterior de esta funcion. Con puntuacion z estos
    /// numeros daban 1,37 y pasaban por experiencia relevante; la diferencia real es de
    /// 0,0014, que es ruido.
    #[test]
    fn resultados_indistinguibles_no_destacan() {
        let similarities = vec![0.851, 0.850, 0.849, 0.850, 0.848, 0.851];
        let medida = standout(&similarities);
        assert!(
            medida < WEAK_STANDOUT,
            "despegue de {medida:.4}: no deberia contar como experiencia relevante"
        );
    }

    #[test]
    fn un_empate_absoluto_no_destaca() {
        assert_eq!(standout(&[0.85, 0.85, 0.85, 0.85]), 0.0);
    }

    #[test]
    fn un_solo_resultado_no_tiene_con_que_compararse() {
        assert_eq!(standout(&[0.99]), 0.0);
        assert_eq!(standout(&[]), 0.0);
    }

    /// El caso que protege §6: que la similitud absoluta sea alta no basta. Si todo el
    /// corpus puntua alto, ninguna experiencia es "la" relevante.
    #[test]
    fn similitud_alta_pero_uniforme_no_es_experiencia_relevante() {
        let todo_alto = vec![0.94, 0.939, 0.938, 0.9385, 0.939];
        assert!(
            standout(&todo_alto) < WEAK_STANDOUT,
            "0,94 de similitud no significa nada si todo el corpus puntua 0,94"
        );
    }

    /// La escala tiene que sobrevivir: dos conjuntos con la misma forma pero distinta
    /// separacion no pueden puntuar igual. Es justo lo que rompia la puntuacion z.
    #[test]
    fn la_medida_conserva_la_escala() {
        let apinado = standout(&[0.8510, 0.8500, 0.8495, 0.8505]);
        let separado = standout(&[0.9100, 0.8500, 0.8495, 0.8505]);
        assert!(
            separado > apinado * 10.0,
            "apinado {apinado:.4} vs separado {separado:.4}: la escala se perdio"
        );
    }
}
