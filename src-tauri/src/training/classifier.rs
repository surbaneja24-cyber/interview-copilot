//! Clasificar la pregunta del entrevistador (§7).
//!
//! Es la pieza que decide dos cosas durante la entrevista: **con que material se contesta**
//! —§5.2 dejo `General` abierto justo esperando esto— y **con que forma** se redacta la
//! sugerencia. Vive junto a la taxonomia y no en `llm/` porque lo que produce es un
//! `QuestionKind`, que se define aqui: separarlos seria tener el vocabulario en un sitio y
//! quien lo usa en otro.
//!
//! ## Reglas primero, y el LLM solo cuando hace falta
//!
//! Es la decision del cuadro de riesgos: clasificar con el LLM anade una pasada entera al
//! camino critico, y la latencia es el punto mas delicado del producto (§10). Una pregunta de
//! entrevista es de las pocas cosas de este dominio con formulas fijas —"cuentame una vez
//! que…", "que harias si…", "cuales son tus expectativas salariales"—, asi que las reglas
//! contestan la mayoria en microsegundos y el modelo se reserva para lo que de verdad es
//! ambiguo.
//!
//! **Para que eso sea cierto tiene que haber un "no se".** Un clasificador que siempre
//! devuelve algo convierte "el LLM resuelve la ambiguedad" en una frase sin contenido: no
//! habria ambiguedad que resolver, habria un valor por defecto con otro nombre. Por eso
//! `SIN_TIPO` es un corpus de control con el mismo peso que el de aciertos.
//!
//! ## Ni un umbral que calibrar
//!
//! Cada patron que encaja suma un punto a su tipo. Gana el que mas puntos tenga. Y hay dos
//! unicas formas de quedarse sin respuesta, ninguna con numero dentro:
//!
//! - **nadie puntua** — no se parece a ninguna de las seis;
//! - **empate arriba** — se parece a dos por igual, que es lo que significa ambiguo.
//!
//! Se eligio asi a proposito. La alternativa era pesar los patrones, y un peso es una
//! constante puesta a ojo de las que este proyecto ya ha pagado dos veces. Cuando un patron
//! generico se comia a uno especifico, la solucion fue **hacer el generico mas estrecho**, no
//! darle menos peso: "cuentame un…" no dice nada por si solo, "cuentame una vez que…" si.
//!
//! ## Los acentos no cuentan aqui
//!
//! Al reves que en el WER de §4.4, donde "años" y "anos" son palabras distintas y la
//! diferencia es justo lo que se mide. Aqui el texto llega de whisper, que a veces se los
//! come, y ninguna de las seis clases depende de una tilde: perder la clasificacion de una
//! pregunta por un acento seria cambiar precision por nada.

use super::QuestionKind;

/// Lo que sale de mirar una pregunta.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Classification {
    /// `None` cuando las reglas no se mojan. No es un fallo: es **el caso que el LLM
    /// resuelve**, y por eso no hay aqui ningun `needs_llm()` de conveniencia — el dia que
    /// exista ese camino, lo escribira quien lo use.
    pub kind: Option<QuestionKind>,
    /// Patrones que encajaron con el tipo ganador. Cero significa que no gano nadie.
    pub matches: usize,
    /// Si hubo empate arriba. Distinto de "no encajo nada", y lleva a sitios distintos:
    /// aqui el LLM tiene dos candidatos entre los que elegir y alli no tiene ninguno.
    pub tied: bool,
}

impl Classification {
    fn nothing() -> Self {
        Self { kind: None, matches: 0, tied: false }
    }
}

/// Un patron y el tipo al que apunta. El texto ya viene normalizado.
type Pattern = (&'static str, QuestionKind);

/// Los patrones, agrupados por tipo para poder leerlos.
///
/// Todos son trozos literales que aparecen dentro de la pregunta, no expresiones regulares.
/// Una expresion regular aqui seria mas corta de escribir y mucho mas dificil de auditar
/// cuando una pregunta salga mal clasificada, que es la operacion que de verdad se va a
/// repetir.
const PATTERNS: &[Pattern] = &[
    // --- Logistics: vocabulario, y es el mas fiable de los seis -------------------------
    ("salario", QuestionKind::Logistics),
    ("sueldo", QuestionKind::Logistics),
    ("banda salarial", QuestionKind::Logistics),
    ("expectativas economicas", QuestionKind::Logistics),
    ("expectativas salariales", QuestionKind::Logistics),
    ("salarial", QuestionKind::Logistics),
    ("cuanto quieres ganar", QuestionKind::Logistics),
    ("cuanto esperas cobrar", QuestionKind::Logistics),
    ("disponibilidad", QuestionKind::Logistics),
    ("incorporarte", QuestionKind::Logistics),
    ("incorporacion", QuestionKind::Logistics),
    ("podrias empezar", QuestionKind::Logistics),
    ("puedes empezar", QuestionKind::Logistics),
    ("horario", QuestionKind::Logistics),
    ("turnos", QuestionKind::Logistics),
    ("jornada", QuestionKind::Logistics),
    ("vacaciones", QuestionKind::Logistics),
    ("preaviso", QuestionKind::Logistics),
    ("carnet de conducir", QuestionKind::Logistics),
    ("vehiculo propio", QuestionKind::Logistics),
    ("coche propio", QuestionKind::Logistics),
    ("trasladarte", QuestionKind::Logistics),
    ("mudarte", QuestionKind::Logistics),
    ("teletrabajo", QuestionKind::Logistics),
    ("otro proceso", QuestionKind::Logistics),
    ("otros procesos", QuestionKind::Logistics),
    // --- SelfAssessment: sobre uno mismo ------------------------------------------------
    ("puntos fuertes", QuestionKind::SelfAssessment),
    ("puntos debiles", QuestionKind::SelfAssessment),
    ("punto debil", QuestionKind::SelfAssessment),
    ("punto fuerte", QuestionKind::SelfAssessment),
    ("fortalezas", QuestionKind::SelfAssessment),
    ("debilidades", QuestionKind::SelfAssessment),
    ("tu mayor defecto", QuestionKind::SelfAssessment),
    ("tus defectos", QuestionKind::SelfAssessment),
    ("te describirias", QuestionKind::SelfAssessment),
    ("tienes que mejorar", QuestionKind::SelfAssessment),
    ("puedes mejorar", QuestionKind::SelfAssessment),
    ("mejorarias de ti", QuestionKind::SelfAssessment),
    ("dirian de ti", QuestionKind::SelfAssessment),
    ("dirian tus companeros", QuestionKind::SelfAssessment),
    ("diria tu jefe", QuestionKind::SelfAssessment),
    ("un error que hayas", QuestionKind::SelfAssessment),
    ("un fracaso", QuestionKind::SelfAssessment),
    ("te arrepientes", QuestionKind::SelfAssessment),
    // --- Motivation: por que nosotros, por que este puesto, por que te vas --------------
    ("por que quieres", QuestionKind::Motivation),
    ("por que te interesa", QuestionKind::Motivation),
    ("por que deberiamos", QuestionKind::Motivation),
    ("por que dejaste", QuestionKind::Motivation),
    ("por que te fuiste", QuestionKind::Motivation),
    ("por que has aplicado", QuestionKind::Motivation),
    ("por que te apuntaste", QuestionKind::Motivation),
    ("te llamo la atencion", QuestionKind::Motivation),
    ("que te atrae", QuestionKind::Motivation),
    ("que sabes de nosotros", QuestionKind::Motivation),
    ("que sabes de la empresa", QuestionKind::Motivation),
    ("donde te ves", QuestionKind::Motivation),
    ("que esperas encontrar", QuestionKind::Motivation),
    ("que esperas de este puesto", QuestionKind::Motivation),
    ("que buscas en", QuestionKind::Motivation),
    ("pregunta para nosotros", QuestionKind::Motivation),
    ("preguntas para nosotros", QuestionKind::Motivation),
    // --- Situational: hipoteticas -------------------------------------------------------
    ("que harias si", QuestionKind::Situational),
    ("que harias en", QuestionKind::Situational),
    ("que harias ante", QuestionKind::Situational),
    ("como actuarias", QuestionKind::Situational),
    ("como reaccionarias", QuestionKind::Situational),
    ("como lo gestionarias", QuestionKind::Situational),
    ("imagina que", QuestionKind::Situational),
    ("imaginate que", QuestionKind::Situational),
    ("supongamos que", QuestionKind::Situational),
    ("pongamos que", QuestionKind::Situational),
    ("supon que", QuestionKind::Situational),
    ("si detectaras", QuestionKind::Situational),
    ("si te encontraras", QuestionKind::Situational),
    ("si tuvieras que", QuestionKind::Situational),
    ("si te cambiaran", QuestionKind::Situational),
    ("que harias con", QuestionKind::Situational),
    ("que haces cuando", QuestionKind::Situational),
    ("que sueles hacer cuando", QuestionKind::Situational),
    // --- Behavioral: una historia concreta que ya paso ----------------------------------
    //
    // Todos llevan una marca de episodio: "una vez", "una situacion", "un ejemplo". Sin
    // ella, "cuentame un…" encaja con media entrevista y no distingue nada.
    ("una vez que", QuestionKind::Behavioral),
    ("la ultima vez que", QuestionKind::Behavioral),
    ("alguna vez que", QuestionKind::Behavioral),
    ("alguna vez has", QuestionKind::Behavioral),
    ("alguna ocasion", QuestionKind::Behavioral),
    ("una situacion en la que", QuestionKind::Behavioral),
    ("una situacion en la que te", QuestionKind::Behavioral),
    ("un momento en el que", QuestionKind::Behavioral),
    ("dame un ejemplo", QuestionKind::Behavioral),
    ("ponme un ejemplo", QuestionKind::Behavioral),
    ("un ejemplo de", QuestionKind::Behavioral),
    ("recuerdas alguna", QuestionKind::Behavioral),
    ("descríbeme una situacion", QuestionKind::Behavioral),
    ("describeme una situacion", QuestionKind::Behavioral),
    ("en el que hayas trabajado", QuestionKind::Behavioral),
    ("que hayas tomado", QuestionKind::Behavioral),
    ("que tuviste que", QuestionKind::Behavioral),
    ("tuviste un conflicto", QuestionKind::Behavioral),
    ("un dia que", QuestionKind::Behavioral),
    ("que hayas tenido que", QuestionKind::Behavioral),
    ("algo que hayas", QuestionKind::Behavioral),
    // --- Experience: que sabe hacer y con que lo ha hecho -------------------------------
    ("un poco sobre ti", QuestionKind::Experience),
    ("hablame de ti", QuestionKind::Experience),
    ("presentate", QuestionKind::Experience),
    ("que herramientas", QuestionKind::Experience),
    ("que programas", QuestionKind::Experience),
    ("que tecnologias", QuestionKind::Experience),
    ("con que herramientas", QuestionKind::Experience),
    ("con que programas", QuestionKind::Experience),
    ("has trabajado con", QuestionKind::Experience),
    ("tienes experiencia", QuestionKind::Experience),
    ("cuanto tiempo llevas", QuestionKind::Experience),
    ("cuantos anos llevas", QuestionKind::Experience),
    ("un dia normal", QuestionKind::Experience),
    ("un dia cualquiera", QuestionKind::Experience),
    ("en que consiste tu trabajo", QuestionKind::Experience),
    ("nivel de ingles", QuestionKind::Experience),
    ("que estudiaste", QuestionKind::Experience),
    ("tu formacion", QuestionKind::Experience),
    ("manejas", QuestionKind::Experience),
    ("dominas", QuestionKind::Experience),
    ("sueles trabajar", QuestionKind::Experience),
];

/// Baja a minusculas y quita los acentos, para que la clasificacion no dependa de que
/// whisper los ponga.
fn normaliza(texto: &str) -> String {
    texto
        .to_lowercase()
        .chars()
        .map(|caracter| match caracter {
            'á' => 'a',
            'é' => 'e',
            'í' => 'i',
            'ó' => 'o',
            'ú' | 'ü' => 'u',
            otro => otro,
        })
        .collect()
}

/// Mira una pregunta y dice de que tipo es, o que no se moja.
pub fn classify(question: &str) -> Classification {
    let texto = normaliza(question);

    // Seis contadores, uno por clase. Un array y no un mapa: son seis y estan fijas.
    let mut puntos = [0usize; 6];
    for (patron, kind) in PATTERNS {
        if texto.contains(patron) {
            puntos[indice(*kind)] += 1;
        }
    }

    let maximo = *puntos.iter().max().expect("son seis");
    if maximo == 0 {
        return Classification::nothing();
    }

    let empatados: Vec<usize> = puntos
        .iter()
        .enumerate()
        .filter(|(_, p)| **p == maximo)
        .map(|(i, _)| i)
        .collect();

    if empatados.len() > 1 {
        return Classification { kind: None, matches: maximo, tied: true };
    }

    Classification {
        kind: Some(desde_indice(empatados[0])),
        matches: maximo,
        tied: false,
    }
}

/// Las seis clases se enumeran a mano en los dos sentidos, sin `as usize` ni `transmute`.
/// Anadir una septima rompe la compilacion aqui, que es donde tiene que romperse.
const fn indice(kind: QuestionKind) -> usize {
    match kind {
        QuestionKind::Behavioral => 0,
        QuestionKind::Motivation => 1,
        QuestionKind::Experience => 2,
        QuestionKind::Situational => 3,
        QuestionKind::SelfAssessment => 4,
        QuestionKind::Logistics => 5,
    }
}

fn desde_indice(indice: usize) -> QuestionKind {
    match indice {
        0 => QuestionKind::Behavioral,
        1 => QuestionKind::Motivation,
        2 => QuestionKind::Experience,
        3 => QuestionKind::Situational,
        4 => QuestionKind::SelfAssessment,
        _ => QuestionKind::Logistics,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::training::corpus::{EVALUACION, SIN_TIPO};
    use crate::training::QUESTIONS;

    /// La unica pregunta del banco con la que el clasificador no se moja, y esta aqui con
    /// nombre y motivo en vez de escondida en un margen de tolerancia.
    ///
    /// *"¿De que logro profesional estas mas orgulloso?"*. El banco la tiene como
    /// `Experience`; al escribir el corpus sellado, y sin haber mirado el banco, la casi
    /// identica *"¿De que trabajo te sientes mas orgulloso?"* salio etiquetada como
    /// `SelfAssessment`. **Dos personas etiquetando lo mismo de dos formas distintas es la
    /// definicion operativa de ambiguo**, asi que la regla se quito y las dos van al modelo.
    /// Inventar un desempate a mano habria sido decidir a ojo lo que ni el propio proyecto
    /// tiene decidido.
    const ABSTENCION_CONSENTIDA: &str = "logro";

    /// Cuenta aciertos, fallos y abstenciones sobre un corpus etiquetado.
    fn mide(corpus: &[(&str, QuestionKind)], titulo: &str) -> (usize, usize, usize) {
        let (mut bien, mut mal, mut sin_mojarse) = (0, 0, 0);

        println!("=== {titulo} ===");
        for (pregunta, esperado) in corpus {
            let salida = classify(pregunta);
            match salida.kind {
                Some(kind) if kind == *esperado => bien += 1,
                Some(kind) => {
                    mal += 1;
                    println!("  FALLO   {esperado:?} -> {kind:?}   \"{pregunta}\"");
                }
                None => {
                    sin_mojarse += 1;
                    let motivo = if salida.tied { "empate" } else { "nada encaja" };
                    println!("  AL LLM  {esperado:?} ({motivo})   \"{pregunta}\"");
                }
            }
        }
        println!(
            "  --> {bien} bien, {mal} mal, {sin_mojarse} al LLM, de {}\n",
            corpus.len()
        );
        (bien, mal, sin_mojarse)
    }

    /// El corpus de desarrollo: las veinte del banco, con el tipo que ya llevaban.
    ///
    /// Acertar aqui demuestra poco —las reglas se escribieron mirandolas— pero fallar
    /// demuestra mucho: si el clasificador no sabe con las preguntas que la propia
    /// aplicacion escribio, no hay nada que medir mas alla.
    #[test]
    fn el_banco_se_clasifica_entero() {
        let corpus: Vec<(&str, QuestionKind)> =
            QUESTIONS.iter().map(|q| (q.text, q.kind)).collect();
        let (bien, mal, _) = mide(&corpus, "BANCO (desarrollo)");

        assert_eq!(mal, 0, "hay preguntas del banco clasificadas en el tipo equivocado");

        // Las abstenciones se enumeran a mano. Una nueva rompe el test, que es el punto:
        // dejar de mojarse con una pregunta que antes se resolvia por reglas es una
        // regresion de latencia silenciosa — sigue contestando bien, pero pagando el modelo.
        let abstenciones: Vec<&str> = QUESTIONS
            .iter()
            .filter(|q| classify(q.text).kind.is_none())
            .map(|q| q.id)
            .collect();
        assert_eq!(
            abstenciones,
            vec![ABSTENCION_CONSENTIDA],
            "las abstenciones del banco han cambiado"
        );
        assert_eq!(bien + abstenciones.len(), QUESTIONS.len());
    }

    /// **El corpus sellado.** Preguntas de entrevista de verdad, escritas antes que las
    /// reglas y sin volver a tocarlas.
    ///
    /// No se exige un porcentaje bonito: se exige que **ningun fallo pase por acierto**. La
    /// abstencion no cuenta como error porque tiene salida —la resuelve el LLM— y una
    /// clasificacion equivocada no la tiene: se lleva por delante el material con el que se
    /// contesta sin que nadie se entere.
    #[test]
    fn el_corpus_sellado() {
        let (bien, mal, al_llm) = mide(EVALUACION, "EVALUACION (sellado)");
        let total = EVALUACION.len();

        println!(
            "aciertos {bien}/{total} · equivocadas {mal} · al LLM {al_llm}  \
             ({}% resuelto sin modelo)",
            (bien + mal) * 100 / total
        );

        assert!(
            bien * 2 > total,
            "las reglas aciertan {bien} de {total}: por debajo de la mitad no ahorran la \
             pasada del modelo, que es su unica razon de existir"
        );
        assert!(
            mal * 5 <= total,
            "{mal} de {total} salen con el tipo equivocado, y una equivocada no tiene \
             salida: cambia el material con el que se contesta y nadie se entera"
        );
    }

    /// **El control, y es el que sostiene la arquitectura.**
    ///
    /// Si esto falla, "reglas primero y el LLM solo ante ambiguedad" es una frase vacia: no
    /// habria ambiguedad que detectar, habria un valor por defecto con nombre de decision.
    #[test]
    fn lo_que_no_es_una_pregunta_de_entrevista_no_recibe_tipo() {
        for frase in SIN_TIPO {
            let salida = classify(frase);
            assert_eq!(
                salida.kind, None,
                "\"{frase}\" no es ninguna de las seis y salio como {:?}",
                salida.kind
            );
        }
    }

    /// Sin acentos tiene que dar lo mismo: el texto llega de whisper, que a veces se los come.
    #[test]
    fn los_acentos_no_cambian_la_clasificacion() {
        for (pregunta, _) in EVALUACION {
            let sin_acentos = normaliza(pregunta);
            assert_eq!(
                classify(pregunta).kind,
                classify(&sin_acentos).kind,
                "\"{pregunta}\" cambia de tipo al perder los acentos"
            );
        }
    }

    #[test]
    fn una_pregunta_vacia_no_recibe_tipo() {
        assert_eq!(classify("").kind, None);
        assert_eq!(classify("   ").kind, None);
    }

    /// El empate se distingue de "no encaja nada". Los dos van al modelo, pero llegan con
    /// informacion distinta: con dos candidatos o con ninguno.
    #[test]
    fn el_empate_se_distingue_de_no_encontrar_nada() {
        let nada = classify("¿me oyes bien?");
        assert!(!nada.tied);
        assert_eq!(nada.matches, 0);

        // Una que dispara Experience y Behavioral a la vez, a proposito.
        let empate = classify("Háblame de ti y ponme un ejemplo de un día normal");
        assert!(empate.tied, "salio {empate:?}");
        assert_eq!(empate.kind, None);
    }
}
