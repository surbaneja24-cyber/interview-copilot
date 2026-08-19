//! Comparacion de modelos de embeddings sobre un corpus de entrevista.
//!
//! No es un test de correccion, es la medicion que respalda la eleccion de modelo. Se
//! corre a mano cuando se plantea cambiarlo:
//!
//! ```text
//! cargo test --lib -- --ignored --nocapture --test-threads=1 benchmark
//! ```
//!
//! Lo que se mide y por que:
//!
//! - **Acierto en top-1**: de cuantas preguntas el fragmento correcto sale el primero.
//!   Es lo unico que importa de verdad, porque el retriever solo pasa los mejores al LLM.
//! - **Margen**: cuanta similitud separa al fragmento correcto del mejor distractor. Un
//!   margen amplio permite poner el umbral de "no tengo experiencia relevante" (§6 del
//!   spec) en un sitio donde de verdad distinga; un margen estrecho lo vuelve arbitrario.

#![cfg(test)]

use super::local::{
    LocalEmbeddingProvider, ModelSpec, E5_SMALL_SIN_PREFIJOS, MULTILINGUAL_E5_BASE,
    MULTILINGUAL_E5_SMALL, PARAPHRASE_ML_MINILM_Q, PARAPHRASE_ML_MPNET,
};
use super::EmbeddingProvider;

/// Fragmentos de un CV ficticio pero realista, en espanol, con distractores a proposito:
/// varios hablan de trabajo en equipo y varios de tecnologia, para que acertar no sea
/// cuestion de coincidencia de palabras sueltas.
const CORPUS: &[&str] = &[
    // 0 — liderazgo tecnico
    "Lideré la migración de un monolito PHP a microservicios en Node durante seis meses, \
     coordinando a cuatro personas y reduciendo el despliegue de dos horas a once minutos.",
    // 1 — conflicto interpersonal
    "Tuve un desacuerdo fuerte con un compañero de backend sobre el diseño de la API. \
     Acabamos escribiendo los dos una propuesta y llevándolas al equipo, y se eligió una mezcla.",
    // 2 — fracaso
    "Un proyecto de recomendador que monté no llegó a producción: subestimé el coste de \
     etiquetar los datos y lo paramos tras dos meses. Aprendí a validar el dato antes que el modelo.",
    // 3 — formación
    "Bootcamp full-stack en 4Geeks: React, Node, Express y PostgreSQL. Proyecto final de \
     un SaaS de gestión de inventario.",
    // 4 — trabajo no técnico
    "Trabajé dos veranos en una cafetería llevando la caja y la atención al cliente en horas punta.",
    // 5 — enseñanza
    "Di clases particulares de matemáticas a estudiantes de bachillerato durante tres años.",
];

/// Cada pregunta con el indice del fragmento que deberia recuperar.
const QUESTIONS: &[(&str, usize)] = &[
    (
        "Cuéntame un proyecto técnico complicado que hayas liderado",
        0,
    ),
    ("Háblame de un conflicto con un compañero de trabajo", 1),
    ("¿Cuál ha sido tu mayor fracaso profesional?", 2),
    ("¿Qué formación tienes en desarrollo web?", 3),
    ("¿Tienes experiencia explicando cosas a otras personas?", 5),
    ("Dime una situación en la que trataste con clientes", 4),
];

/// Preguntas cuya respuesta **no** esta en el corpus. Sin ellas no se puede calibrar el
/// aviso de §6: solo con ejemplos positivos, cualquier umbral parece bueno.
const QUESTIONS_WITHOUT_ANSWER: &[&str] = &[
    "¿Qué experiencia tienes administrando clústeres de Kubernetes en producción?",
    "Háblame de cuando gestionaste un presupuesto de marketing de seis cifras",
    "¿Has trabajado alguna vez redactando contratos mercantiles?",
    "Cuéntame tu experiencia dirigiendo un equipo de ventas internacional",
];

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    dot / (norm_a * norm_b)
}

struct Score {
    top1: usize,
    total: usize,
    mean_margin: f32,
}

fn evaluate(spec: &'static ModelSpec) -> Option<Score> {
    let cache = std::env::temp_dir().join("interview-copilot-models");
    let provider = match LocalEmbeddingProvider::with_model(&cache, spec) {
        Ok(provider) => provider,
        Err(err) => {
            // Que un candidato no cargue no puede tumbar la comparacion de los demas.
            println!("\n=== {} ===\n  NO CARGA: {err}", spec.id);
            return None;
        }
    };

    println!("\n=== {} ===", spec.id);

    let docs: Vec<String> = CORPUS.iter().map(|text| (*text).to_owned()).collect();
    // Un modelo puede cargar y fallar al inferir: el ONNX cuantizado de Qdrant lo hace
    // con las versiones nuevas de ONNX Runtime. Tampoco eso puede tumbar la comparacion.
    let vectors = match provider.embed_documents(&docs) {
        Ok(vectors) => vectors,
        Err(err) => {
            println!("  NO INFIERE: {err}");
            return None;
        }
    };

    let mut top1 = 0usize;
    let mut margins = Vec::new();

    for (question, expected) in QUESTIONS {
        let Ok(query) = provider.embed_query(question) else {
            println!("  NO INFIERE la consulta");
            return None;
        };

        let mut scored: Vec<(usize, f32)> = vectors
            .iter()
            .enumerate()
            .map(|(index, vector)| (index, cosine(&query, vector)))
            .collect();
        scored.sort_by(|a, b| b.1.total_cmp(&a.1));

        let (best_index, best_score) = scored[0];
        let correct = best_index == *expected;
        if correct {
            top1 += 1;
        }

        // Margen entre el correcto y el mejor que no lo es.
        let correct_score = scored
            .iter()
            .find(|(index, _)| index == expected)
            .map_or(0.0, |(_, score)| *score);
        let best_wrong = scored
            .iter()
            .find(|(index, _)| index != expected)
            .map_or(0.0, |(_, score)| *score);
        let margin = correct_score - best_wrong;
        margins.push(margin);

        println!(
            "  {} \"{}\" → {} (margen {:+.4})",
            if correct { "OK  " } else { "FALLA" },
            question,
            best_index,
            margin
        );
        let _ = best_score;
    }

    let mean_margin = margins.iter().sum::<f32>() / margins.len() as f32;
    println!(
        "  top-1: {}/{} · margen medio {:+.4}",
        top1,
        QUESTIONS.len(),
        mean_margin
    );

    Some(Score {
        top1,
        total: QUESTIONS.len(),
        mean_margin,
    })
}

/// Calibra el umbral de "no encuentro experiencia relevante" (§6) con datos.
///
/// Mide el despegue —similitud del mejor resultado menos la media del resto— para
/// preguntas que si tienen respuesta en el corpus y para preguntas que no. El umbral
/// util es cualquier valor que quede entre las dos nubes; si se solapan, este corpus no
/// permite distinguir y hay que decirlo en vez de fingir que el aviso funciona.
#[test]
#[ignore = "descarga modelos y tarda"]
fn calibra_el_umbral_de_experiencia_relevante() {
    let cache = std::env::temp_dir().join("interview-copilot-models");
    let provider = LocalEmbeddingProvider::with_model(&cache, super::local::DEFAULT_MODEL)
        .expect("cargar el modelo por defecto");

    let docs: Vec<String> = CORPUS.iter().map(|text| (*text).to_owned()).collect();
    let vectors = provider.embed_documents(&docs).expect("embeder corpus");

    // Dos senales candidatas por pregunta: el despegue del mejor sobre la media del
    // resto, y la similitud absoluta del mejor.
    let medir = |question: &str| -> (f32, f32) {
        let query = provider.embed_query(question).expect("embeder pregunta");
        let mut scores: Vec<f32> = vectors.iter().map(|v| cosine(&query, v)).collect();
        scores.sort_by(|a, b| b.total_cmp(a));
        let (best, rest) = scores.split_first().expect("corpus no vacio");
        (best - rest.iter().sum::<f32>() / rest.len() as f32, *best)
    };

    println!("\n{:<12} {:<10} pregunta", "despegue", "absoluta");

    println!("\nCON respuesta en el corpus:");
    let mut positivos = Vec::new();
    for (question, _) in QUESTIONS {
        let (despegue, absoluta) = medir(question);
        positivos.push((despegue, absoluta));
        println!("  {despegue:+.4}      {absoluta:.4}     {question}");
    }

    println!("\nSIN respuesta en el corpus:");
    let mut negativos = Vec::new();
    for question in QUESTIONS_WITHOUT_ANSWER {
        let (despegue, absoluta) = medir(question);
        negativos.push((despegue, absoluta));
        println!("  {despegue:+.4}      {absoluta:.4}     {question}");
    }

    reporta_separacion("despegue", &positivos, &negativos, |par| par.0);
    reporta_separacion("similitud absoluta", &positivos, &negativos, |par| par.1);
}

/// Dice si una senal separa las dos nubes y, si lo hace, en que intervalo cae el umbral.
fn reporta_separacion(
    nombre: &str,
    positivos: &[(f32, f32)],
    negativos: &[(f32, f32)],
    extraer: impl Fn(&(f32, f32)) -> f32,
) {
    let min_positivo = positivos.iter().map(&extraer).fold(f32::MAX, f32::min);
    let max_negativo = negativos.iter().map(&extraer).fold(f32::MIN, f32::max);

    println!(
        "\n  [{nombre}] positivo mas bajo {min_positivo:.4} · negativo mas alto {max_negativo:.4}"
    );

    if min_positivo > max_negativo {
        println!(
            "  SEPARA: umbral valido en ({max_negativo:.4}, {min_positivo:.4}), punto medio {:.4}",
            (min_positivo + max_negativo) / 2.0
        );
    } else {
        println!("  NO SEPARA: las nubes se solapan.");
    }
}

/// Ultima carta para el aviso de §6: un cross-encoder.
///
/// La similitud coseno compara dos vectores calculados por separado, asi que mide "de que
/// habla cada texto" y no "responde este texto a esta pregunta". Un cross-encoder procesa
/// el par junto y puntua exactamente lo segundo. Aqui se comprueba si eso basta para
/// separar las preguntas con respuesta de las que no la tienen.
#[test]
#[ignore = "descarga modelos grandes y tarda"]
fn calibra_el_umbral_con_reranker() {
    use fastembed::{RerankInitOptions, RerankerModel, TextRerank};

    let cache = std::env::temp_dir().join("interview-copilot-models");
    let docs: Vec<&str> = CORPUS.to_vec();

    for modelo in [
        RerankerModel::JINARerankerV2BaseMultiligual,
        RerankerModel::BGERerankerV2M3,
    ] {
        println!("\n=== {modelo} ===");

        let options = RerankInitOptions::new(modelo.clone())
            .with_cache_dir(cache.clone())
            .with_show_download_progress(true);

        let reranker = match TextRerank::try_new(options) {
            Ok(reranker) => reranker,
            Err(err) => {
                println!("  NO CARGA: {err}");
                continue;
            }
        };

        let mejor = |question: &str| -> Option<(f32, usize)> {
            let results = reranker.rerank(question, docs.clone(), false, None).ok()?;
            let best = results.first()?;
            Some((best.score, best.index))
        };

        println!("\n  CON respuesta en el corpus:");
        let mut positivos = Vec::new();
        let mut aciertos = 0usize;
        for (question, expected) in QUESTIONS {
            let Some((score, index)) = mejor(question) else {
                println!("    NO INFIERE");
                continue;
            };
            if index == *expected {
                aciertos += 1;
            }
            positivos.push(score);
            println!(
                "    {score:>10.4}  →{index}  {}  {question}",
                if index == *expected { "OK  " } else { "FALLA" }
            );
        }

        println!("\n  SIN respuesta en el corpus:");
        let mut negativos = Vec::new();
        for question in QUESTIONS_WITHOUT_ANSWER {
            let Some((score, index)) = mejor(question) else {
                println!("    NO INFIERE");
                continue;
            };
            negativos.push(score);
            println!("    {score:>10.4}  →{index}  {question}");
        }

        if positivos.is_empty() || negativos.is_empty() {
            continue;
        }

        let min_positivo = positivos.iter().copied().fold(f32::MAX, f32::min);
        let max_negativo = negativos.iter().copied().fold(f32::MIN, f32::max);

        println!(
            "\n  top-1 {aciertos}/{} · positivo mas bajo {min_positivo:.4} · negativo mas alto {max_negativo:.4}",
            QUESTIONS.len()
        );

        if min_positivo > max_negativo {
            println!(
                "  SEPARA: umbral valido en ({max_negativo:.4}, {min_positivo:.4}), punto medio {:.4}",
                (min_positivo + max_negativo) / 2.0
            );
        } else {
            println!("  NO SEPARA: las nubes se solapan.");
        }
    }
}

/// Diagnostico: matriz de similitud entre documentos del corpus.
///
/// Si todo se parece a todo por encima de ~0.97, los vectores son degenerados y el
/// problema esta en como se generan, no en que modelo se eligio. Si hay dispersion, el
/// modelo distingue y lo que falla es la consulta.
#[test]
#[ignore = "descarga modelos y tarda"]
fn matriz_de_similitud_del_corpus() {
    let cache = std::env::temp_dir().join("interview-copilot-models");
    let provider = LocalEmbeddingProvider::with_model(&cache, &MULTILINGUAL_E5_SMALL)
        .expect("cargar el modelo");

    let docs: Vec<String> = CORPUS.iter().map(|text| (*text).to_owned()).collect();
    let vectors = provider.embed_documents(&docs).expect("embeder corpus");

    println!("\nsimilitud documento-documento:");
    print!("      ");
    for index in 0..vectors.len() {
        print!("   {index}   ");
    }
    println!();

    let mut off_diagonal = Vec::new();
    for (i, a) in vectors.iter().enumerate() {
        print!("  {i}:  ");
        for (j, b) in vectors.iter().enumerate() {
            let sim = cosine(a, b);
            print!("{sim:.3}  ");
            if i != j {
                off_diagonal.push(sim);
            }
        }
        println!();
    }

    let min = off_diagonal.iter().copied().fold(f32::MAX, f32::min);
    let max = off_diagonal.iter().copied().fold(f32::MIN, f32::max);
    let mean = off_diagonal.iter().sum::<f32>() / off_diagonal.len() as f32;
    println!("\n  fuera de la diagonal: min {min:.4} · media {mean:.4} · max {max:.4}");
    println!("  dispersion (max-min): {:.4}", max - min);
}

#[test]
#[ignore = "descarga modelos y tarda"]
fn compara_modelos_multilingues() {
    let candidatos: [&'static ModelSpec; 5] = [
        &MULTILINGUAL_E5_SMALL,
        &E5_SMALL_SIN_PREFIJOS,
        &PARAPHRASE_ML_MINILM_Q,
        &MULTILINGUAL_E5_BASE,
        &PARAPHRASE_ML_MPNET,
    ];

    let mut resultados = Vec::new();
    for spec in candidatos {
        if let Some(score) = evaluate(spec) {
            resultados.push((spec.id, score));
        }
    }

    println!("\nRESUMEN");
    for (id, score) in &resultados {
        println!(
            "  {:<40} top-1 {}/{}  margen medio {:+.4}",
            id, score.top1, score.total, score.mean_margin
        );
    }

    let mejor = resultados
        .iter()
        .max_by_key(|(_, score)| score.top1)
        .expect("al menos un modelo deberia cargar");
    println!("\n  mejor candidato: {}", mejor.0);

    // Guardia de regresion: el modelo por defecto tiene que seguir siendo el mejor de
    // los candidatos. Si alguien lo cambia por uno mas pequeno "porque ocupa menos",
    // esto lo detiene.
    let por_defecto = resultados
        .iter()
        .find(|(id, _)| *id == super::local::DEFAULT_MODEL.id)
        .expect("el modelo por defecto tiene que estar entre los candidatos");

    assert_eq!(
        por_defecto.1.top1, por_defecto.1.total,
        "el modelo por defecto ya no acierta todas las preguntas del corpus"
    );
    assert!(
        por_defecto.1.mean_margin > 0.0,
        "el margen medio del modelo por defecto se ha vuelto negativo"
    );
}
