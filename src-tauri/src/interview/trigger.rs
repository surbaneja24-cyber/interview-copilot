//! Si un turno del entrevistador merece que se prepare una sugerencia.
//!
//! No todo lo que dice el entrevistador es una pregunta. "Vale, perfecto", "un momento que
//! llamo a mi companera" o "¿me oyes bien?" son turnos como cualquier otro para el VAD, y
//! cada uno que se deja pasar cuesta una pasada entera del pipeline — recuperacion mas LLM —
//! justo cuando la latencia es lo mas delicado del producto (§10).
//!
//! ## Lo que esta pieza es y lo que no
//!
//! **No decide si la respuesta va a ser correcta.** De eso ya se encargan §5 y §6, y estan
//! medidos: si el turno no era una pregunta, el retriever no encuentra material y el modelo
//! contesta que no puede. Sale una sugerencia vacia, no una inventada.
//!
//! Lo unico que esta en juego es **una pasada desperdiciada**. Y eso cambia por completo
//! hacia donde hay que equivocarse: saltarse una pregunta de verdad deja al candidato sin
//! ayuda en el peor momento posible, y colar un "vale, perfecto" cuesta unos segundos de CPU
//! y un aviso de "no hay material". No son comparables, asi que el filtro es **conservador a
//! proposito**: solo quita lo que evidentemente no es una pregunta.
//!
//! ## Lo medido
//!
//! Sobre los dos corpus del clasificador (`training::corpus`): **descarta 3 de las 7 frases
//! que no son preguntas de entrevista, y no descarta ninguna de las 32 que si lo son**.
//!
//! Las cuatro que se cuelan son "¿me oyes bien?", "¿puedes ponerte mas cerca del microfono?",
//! "¿que tal el viaje hasta aqui?" y "entonces te llamamos la semana que viene, ¿te parece?".
//! Las cuatro son preguntas de verdad, con interrogacion y todo; lo que no son es preguntas
//! **de entrevista**, y esa distincion no esta en la forma de la frase sino en lo que quiere
//! decir. Separarlas con reglas seria adivinar. Cuestan una pasada cada una y ahi se quedan,
//! escritas como limite y no como fila que falta.

use crate::training::classifier;

/// Palabras minimas para que un turno se considere una pregunta.
///
/// De los dos corpus, igual que los demas umbrales de este proyecto — solo que aqui el hueco
/// es tan estrecho que **no queda nada que elegir**:
///
/// - la pregunta de entrevista mas corta tiene **3 palabras**: "¿cuando podrias
///   incorporarte?";
/// - lo mas largo que hay que descartar tiene **2**: "¿y eso?", "vale, perfecto".
///
/// Entre dos y tres solo cabe un numero entero, asi que la media geometrica que se ha usado
/// para `MIN_TURN_MS` y `MIN_WORDS` aqui no tiene donde caer. Eso no lo hace mas robusto sino
/// menos: el umbral esta pegado al borde, y una pregunta de dos palabras —"¿por que?"— caeria
/// del lado equivocado. Queda anotado como el mas fragil de los tres.
const MIN_QUESTION_WORDS: usize = 3;

/// Como empieza una pregunta de entrevista que no lleva interrogacion.
///
/// Son imperativos, y no son un detalle: cuatro de las 32 del corpus sellado se formulan asi
/// —"ponme un ejemplo de…", "describeme una situacion en la que…"— y sin esta lista se
/// descartarian todas por no llevar signo de interrogacion.
const IMPERATIVOS: &[&str] = &[
    "cuentame", "hablame", "describeme", "descríbeme", "dame", "ponme", "explicame", "cuenta",
    "dime", "comentame",
];

/// Por que no merece la pena preparar nada.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Skip {
    /// Dos palabras o menos: un "vale" o un "¿y eso?".
    TooShort,
    /// Ni interrogacion ni imperativo: es una frase, no una pregunta.
    NotAQuestion,
}

/// Decide si preparar una sugerencia para este turno.
///
/// `continuing` es si ya hay una pregunta a medias a la que este turno se pegaria, y cambia
/// una de las dos reglas. **Un turno que continua una pregunta no tiene por que parecer una
/// pregunta.** Salio al escribir el primer test de la maquina de estados: la ampliacion mas
/// natural del mundo —"Y ponme un ejemplo con cifras si las tienes"— no lleva interrogacion y
/// empieza por "Y", asi que sola no pasa por pregunta, y pegada a la anterior es media
/// pregunta que se estaba perdiendo.
///
/// Lo que si se aplica siempre es el minimo de palabras: un "vale" no es una pregunta ni es
/// una ampliacion de nada, este donde este.
pub fn should_prepare(turn: &str, continuing: bool) -> Result<(), Skip> {
    let texto = turn.trim();
    let palabras = texto
        .split_whitespace()
        .filter(|palabra| palabra.chars().any(char::is_alphanumeric))
        .count();

    if palabras < MIN_QUESTION_WORDS {
        return Err(Skip::TooShort);
    }

    if continuing {
        return Ok(());
    }

    // La interrogacion de apertura basta: whisper la pone en español, y si no la pusiera
    // quedaria la de cierre.
    if texto.contains('?') || texto.contains('¿') {
        return Ok(());
    }

    let primera: String = classifier::sin_acentos(texto)
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .chars()
        .filter(|caracter| caracter.is_alphanumeric())
        .collect();

    if IMPERATIVOS.contains(&primera.as_str()) {
        return Ok(());
    }

    Err(Skip::NotAQuestion)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::training::corpus::{EVALUACION, SIN_TIPO};

    /// **El que importa.** Saltarse una pregunta de verdad deja al candidato sin ayuda
    /// mientras el entrevistador espera, y eso no tiene arreglo por ningun otro lado.
    #[test]
    fn no_se_salta_ninguna_pregunta_de_entrevista() {
        for (pregunta, _) in EVALUACION {
            assert_eq!(
                should_prepare(pregunta, false),
                Ok(()),
                "se descarta una pregunta del corpus sellado: \"{pregunta}\""
            );
        }
    }

    /// Y lo que quita, que es lo que justifica que exista.
    #[test]
    fn descarta_lo_que_evidentemente_no_es_una_pregunta() {
        let descartadas: Vec<&str> = SIN_TIPO
            .iter()
            .copied()
            .filter(|frase| should_prepare(frase, false).is_err())
            .collect();

        println!("descartadas {} de {}:", descartadas.len(), SIN_TIPO.len());
        for frase in &descartadas {
            println!("  {frase}");
        }

        assert_eq!(
            descartadas.len(),
            3,
            "el filtro descarta {} frases y se esperaban 3; si sube, comprueba antes que no \
             se esta llevando por delante alguna pregunta de verdad",
            descartadas.len()
        );
    }

    #[test]
    fn una_pregunta_sin_interrogacion_pero_con_imperativo_pasa() {
        assert_eq!(should_prepare("Cuéntame un poco sobre ti", false), Ok(()));
        assert_eq!(should_prepare("Descríbeme una situación complicada", false), Ok(()));
    }

    #[test]
    fn un_turno_vacio_no_prepara_nada() {
        assert_eq!(should_prepare("", false), Err(Skip::TooShort));
        assert_eq!(should_prepare("   ", false), Err(Skip::TooShort));
    }

    /// El umbral cae entre las dos medidas, y esto lo fija.
    #[test]
    fn el_umbral_de_palabras_cae_entre_los_dos_corpus() {
        let contar = |texto: &str| {
            texto
                .split_whitespace()
                .filter(|p| p.chars().any(char::is_alphanumeric))
                .count()
        };

        let pregunta_mas_corta = EVALUACION
            .iter()
            .map(|(texto, _)| contar(texto))
            .min()
            .expect("hay preguntas");
        println!("la pregunta mas corta del corpus sellado tiene {pregunta_mas_corta} palabras");

        assert_eq!(pregunta_mas_corta, 3, "el corpus sellado ha cambiado");
        assert!(
            MIN_QUESTION_WORDS <= pregunta_mas_corta,
            "{MIN_QUESTION_WORDS} palabras se lleva por delante la pregunta mas corta"
        );
        assert!(MIN_QUESTION_WORDS > contar("¿Y eso?"));
    }

    /// La ampliacion de una pregunta no parece una pregunta, y ese es justo el caso.
    #[test]
    fn una_ampliacion_no_tiene_que_parecer_una_pregunta() {
        const AMPLIACION: &str = "Y ponme un ejemplo con cifras si las tienes";

        assert_eq!(
            should_prepare(AMPLIACION, false),
            Err(Skip::NotAQuestion),
            "sola no es una pregunta, y esta bien que no lo sea"
        );
        assert_eq!(
            should_prepare(AMPLIACION, true),
            Ok(()),
            "pegada a la pregunta anterior si, y perderla es contestar media pregunta"
        );
    }

    /// Pero el minimo de palabras se aplica igual, se este continuando o no.
    #[test]
    fn un_vale_tampoco_amplia_nada() {
        assert_eq!(should_prepare("Vale", true), Err(Skip::TooShort));
    }
}
