//! Mirar una respuesta dictada antes de guardarla.
//!
//! El modo diapositiva quito friccion y de paso quito la ultima oportunidad de ver lo que se
//! estaba archivando: cualquier texto en la caja disparaba la cuenta de 4 s y se guardaba
//! solo. El 2026-08-21 se guardaron asi ocho respuestas inservibles, y el material entrenado
//! le gana la recuperacion al CV por nueve centesimas (§5.1), o sea que la basura queda
//! arriba del todo en todas las entrevistas siguientes.
//!
//! Esto **no puntua respuestas ni decide si una respuesta es buena**. Eso necesita un modelo
//! capaz, que en esta maquina no hay, y fingirlo con un porcentaje seria el error que §12 ya
//! tiene anotado. Lo unico que hace es decidir si la respuesta se guarda sola o hay que
//! mirarla, y ante la duda se mira: un falso positivo cuesta un clic y un falso negativo
//! cuesta el corpus.
//!
//! ## De donde salen las reglas
//!
//! De comparar dos corpus reales, no de imaginar como falla una transcripcion:
//!
//! - **Las ocho respuestas envenenadas del 2026-08-21**, tal cual se guardaron. Estan en
//!   `tests::ENVENENADAS` y se quedan ahi para siempre: son la unica muestra que hay de como
//!   se rompe esto de verdad.
//! - **Las seis frases del corpus de referencia** (`testing::FRASES`), que es como suena una
//!   respuesta correcta del mismo dominio.
//!
//! Medido asi, las cuatro reglas cazan **siete de las ocho** y **ninguna de las seis buenas**.
//! La que se escapa —"Ah, y me voy a estar a ver de ahi, un boque este video."— tiene largo
//! de respuesta, empieza en mayuscula y no lleva ninguna marca: para verla hace falta
//! entender lo que dice, y eso es otro problema.
//!
//! Que ninguna buena salte es el control, y es el que importa: una regla que marca respuestas
//! validas no es una regla, es ruido, y a los tres avisos el usuario deja de leerlos.

use crate::error::AppResult;

/// Palabras por debajo de las cuales una respuesta se mira antes de guardarla.
///
/// **Recalibrado el 2026-08-22 con respuestas de verdad**, que es la primera vez que las hay.
/// Salio a 10 de los dos corpus de laboratorio; con las veintiuna que Santiago dicto de
/// principio a fin, los dos extremos se mueven:
///
/// - la respuesta buena mas corta sigue teniendo **13 palabras**. Las dos fuentes coinciden,
///   y eso es mas de lo que se podia pedir de un corpus sintetico de seis frases;
/// - la respuesta rota mas larga ya no tiene 8 sino **10** — "Ahora mismo. un sistema de
///   bastantes sistema buenos. y bueno" —, que con el umbral viejo pasaba justo por debajo.
///
/// Once es la media geometrica de 10 y 13, la misma regla que `MIN_TURN_MS`: igual de lejos
/// de los dos medida en veces y no en palabras.
///
/// La preocupacion que quedaba anotada aqui —"no hay ni una respuesta legitimamente corta en
/// el corpus"— ya tiene respuesta, y no era la que se temia. Las dos preguntas de
/// `Logistics` se contestaron en 54 y 21 palabras, muy por encima del suelo. Nadie contesta
/// una entrevista en seis palabras aunque la pregunta lo permita.
const MIN_WORDS: usize = 11;

/// Lo que hace sospechar de una respuesta. Sin texto para el usuario: eso es de la UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Reason {
    /// Lleva una marca de no-habla de whisper: `[Música]`, `[Aplausos]`, `♪`.
    NonSpeechMarker,
    /// Demasiado corta para ser una respuesta de entrevista.
    TooShort { words: usize },
    /// Empieza a media frase, que es la firma de haberse comido el arranque.
    StartsMidSentence,
    /// Lleva guiones de dialogo: whisper los pone cuando cree oir a dos personas.
    DialogueDashes,
}

/// El veredicto sobre una respuesta.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnswerReview {
    /// Si hay que pedir confirmacion antes de guardar.
    pub suspicious: bool,
    pub reasons: Vec<Reason>,
    pub words: usize,
}

/// Conjunciones y enlaces con los que **no** empieza una respuesta que empiece por el
/// principio.
///
/// Sale de dos de las ocho envenenadas: "y se un sistema…" y "Y de ultimo lejos se voy…". La
/// segunda lleva mayuscula, asi que mirar la caja de la primera letra no basta — whisper pone
/// mayuscula al empezar aunque lo que oye empiece a media frase.
///
/// **`porque` y `pues` estaban aqui y se han quitado el 2026-08-22.** Con respuestas reales
/// delante marcaban dos buenas de dos: contestar "¿por que deberiamos contratarte?" con
/// "porque considero que…" no es empezar a media frase, es contestar la pregunta que te han
/// hecho.
const ENLACES: &[&str] = &[
    "y", "e", "o", "u", "ni", "que", "pero", "aunque", "sino", "entonces", "asi", "además",
    "ademas", "también", "tambien",
];

/// Marcas de no-habla que pone whisper cuando no hay voz que transcribir.
///
/// Se buscan **los corchetes**, no una lista de palabras. La lista se quedaria corta el dia
/// que el modelo escriba `[Ruido]` en vez de `[Música]`, y un candidato dictando una
/// respuesta no dice corchetes: cualquier cosa entre ellos la ha puesto el modelo, no el
/// usuario. `♪` va aparte porque whisper la usa suelta, sin corchetes.
fn tiene_marca_de_no_habla(texto: &str) -> bool {
    let corchetes = texto.contains('[') && texto.contains(']');
    corchetes || texto.contains('♪')
}

/// Guiones de dialogo. whisper los mete cuando cree estar oyendo una conversacion, y una
/// respuesta dictada a una pregunta es una sola persona hablando.
///
/// Se exige que el guion abra una linea o vaya detras de un espacio, para no confundirlo con
/// un guion de palabra compuesta ni con un rango de fechas.
fn tiene_guiones_de_diliogo(texto: &str) -> bool {
    let empieza = |t: &str| t.starts_with('-') || t.starts_with('—') || t.starts_with('–');
    empieza(texto.trim_start())
        || texto
            .split_whitespace()
            .filter(|palabra| empieza(palabra))
            .count()
            >= 1
}

/// Palabras de verdad: las que tienen alguna letra o cifra. Sin esto, "-¡Claro!" contaria
/// igual que una palabra y los guiones sueltos inflarian la cuenta.
fn palabras(texto: &str) -> usize {
    texto
        .split_whitespace()
        .filter(|palabra| palabra.chars().any(char::is_alphanumeric))
        .count()
}

/// Si la respuesta arranca a media frase.
fn empieza_a_media_frase(texto: &str) -> bool {
    let Some(primera) = texto.split_whitespace().next() else {
        return false;
    };

    let limpia: String = primera
        .chars()
        .filter(|caracter| caracter.is_alphanumeric())
        .collect();
    if limpia.is_empty() {
        return false;
    }

    // **Solo la lista, no la minuscula.**
    //
    // Habia aqui un "empieza en minuscula" que parecia razonable: whisper pone mayuscula, asi
    // que una minuscula seria senal de que el trozo que llego no era el principio. Con
    // respuestas de verdad delante resulto ser falso — whisper deja en minuscula el arranque
    // de muchos turnos sanos— y marcaba **seis de dieciocho respuestas buenas**: "una vez
    // cometi un error…", "solo trabajar bastante con…", "de momento tengo disponibilidad…".
    //
    // Un aviso que salta una de cada tres veces sin motivo deja de leerse, y entonces el
    // filtro no existe aunque siga en el codigo. Quitarlo no pierde ninguna de las ocho
    // envenenadas: las que empezaban a media frase de verdad lo hacen con una conjuncion.
    ENLACES.contains(&limpia.to_lowercase().as_str())
}

/// Mira una respuesta y dice si hay que confirmarla antes de guardarla.
pub fn review(answer: &str) -> AnswerReview {
    let texto = answer.trim();
    let words = palabras(texto);
    let mut reasons = Vec::new();

    if tiene_marca_de_no_habla(texto) {
        reasons.push(Reason::NonSpeechMarker);
    }
    if words < MIN_WORDS {
        reasons.push(Reason::TooShort { words });
    }
    if empieza_a_media_frase(texto) {
        reasons.push(Reason::StartsMidSentence);
    }
    if tiene_guiones_de_diliogo(texto) {
        reasons.push(Reason::DialogueDashes);
    }

    AnswerReview {
        suspicious: !reasons.is_empty(),
        words,
        reasons,
    }
}

/// La misma revision, por IPC. Es una funcion pura: no toca la base ni el disco.
pub fn review_answer(answer: &str) -> AppResult<AnswerReview> {
    Ok(review(answer))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FRASES;

    /// **Las ocho que envenenaron el corpus el 2026-08-21**, copiadas tal cual se guardaron.
    ///
    /// Se quedan aqui para siempre. Son la unica muestra que hay de como se rompe esto de
    /// verdad, y cualquier cambio en las reglas tiene que seguir cazandolas o explicar por
    /// que ya no hace falta.
    const ENVENENADAS: &[&str] = &[
        "Santiago y tengo 21 años.",
        "y se un sistema de inventario para empresas.",
        "[Música]",
        "¡Aguien es bien!",
        "rodaja porque si lo pico de un agua, te burréis. -¿Y aquí está la semilla? -Sí.",
        "-¿Qué es lo que? -No me va a dar a vos, tapis. -¡Claro! -Malano. Y que pueden reaccionar, y los dos.",
        "Ah, y me voy a estar a ver de ahí, un boque este video.",
        "Y de último lejos se voy a ir por aquí porque se voy a ir en este quema.",
    ];

    /// La unica de las ocho que no se puede cazar sin entender lo que dice: tiene largo de
    /// respuesta, empieza en mayuscula y no lleva ninguna marca. Esta escrita aparte para que
    /// el limite del filtro sea explicito y no una fila que falta.
    const LA_QUE_SE_ESCAPA: &str = "Ah, y me voy a estar a ver de ahí, un boque este video.";

    /// **El control, y es el que decide si esto vale.** Una regla que marca respuestas buenas
    /// no es una regla: a los tres avisos el usuario deja de leerlos y el filtro deja de
    /// existir aunque siga en el codigo.
    #[test]
    fn ninguna_respuesta_buena_se_marca() {
        for (pregunta, buena) in FRASES {
            let review = review(buena);
            assert!(
                !review.suspicious,
                "\"{buena}\" es una respuesta correcta a \"{pregunta}\" y salio marcada por \
                 {:?}",
                review.reasons
            );
        }
    }

    #[test]
    fn siete_de_las_ocho_envenenadas_se_cazan() {
        let cazadas: Vec<&str> = ENVENENADAS
            .iter()
            .copied()
            .filter(|texto| review(texto).suspicious)
            .collect();

        assert_eq!(
            cazadas.len(),
            7,
            "de las ocho envenenadas se cazan {} y se esperaban 7",
            cazadas.len()
        );
        assert!(
            !cazadas.contains(&LA_QUE_SE_ESCAPA),
            "se caza la que estaba documentada como no cazable: hay que actualizar el limite"
        );
    }

    /// **El corpus de campo, y es el que manda a partir de ahora.**
    ///
    /// Veintiuna respuestas dictadas de principio a fin, sin nadie escribiendolas para medir
    /// nada. Las tres cifras juntas son el filtro entero: si se cazan las rotas pero se marca
    /// una de cada tres buenas, el aviso deja de leerse y el filtro no existe.
    #[test]
    fn sobre_respuestas_de_verdad() {
        use crate::training::campo::{Estado, RESPUESTAS};

        let (mut rotas_cazadas, mut rotas, mut falsos, mut usables) = (0, 0, 0, 0);

        for (estado, texto) in RESPUESTAS {
            let marcada = review(texto).suspicious;
            match estado {
                Estado::Rota => {
                    rotas += 1;
                    if marcada {
                        rotas_cazadas += 1;
                    } else {
                        println!("  SE ESCAPA  {texto}");
                    }
                }
                Estado::Usable => {
                    usables += 1;
                    if marcada {
                        falsos += 1;
                        println!("  FALSO POSITIVO ({:?})  {texto}", review(texto).reasons);
                    }
                }
            }
        }

        println!("rotas cazadas {rotas_cazadas}/{rotas} · falsos positivos {falsos}/{usables}");

        assert_eq!(rotas_cazadas, rotas, "se escapa una respuesta rota");
        assert_eq!(
            falsos, 0,
            "{falsos} de {usables} respuestas buenas salen marcadas; un aviso que salta sin \
             motivo deja de leerse y entonces el filtro no existe aunque siga en el codigo"
        );
    }

    /// El umbral de palabras vive en el hueco entre las dos respuestas reales que lo fijan.
    ///
    /// Y el limite de abajo no es "la rota mas larga" sino **la rota mas larga que solo la
    /// longitud puede cazar**. Es una distincion que costo un test en rojo: la rota mas larga
    /// de las tres tiene veinticinco palabras, pero empieza por "y" y la caza otra regla, asi
    /// que subir el umbral hasta ahi seria pedirle a la longitud un trabajo que ya esta hecho
    /// — y de paso marcar media docena de respuestas buenas.
    #[test]
    fn el_umbral_de_palabras_cae_entre_las_respuestas_reales() {
        use crate::training::campo::{Estado, RESPUESTAS};

        let mas_corta_buena = RESPUESTAS
            .iter()
            .filter(|(estado, _)| *estado == Estado::Usable)
            .map(|(_, texto)| palabras(texto))
            .min()
            .expect("hay respuestas");

        let solo_por_longitud = RESPUESTAS
            .iter()
            .filter(|(estado, _)| *estado == Estado::Rota)
            .filter(|(_, texto)| {
                !tiene_marca_de_no_habla(texto)
                    && !empieza_a_media_frase(texto)
                    && !tiene_guiones_de_diliogo(texto)
            })
            .map(|(_, texto)| palabras(texto))
            .max()
            .expect("alguna rota depende solo de la longitud");

        println!(
            "la buena mas corta tiene {mas_corta_buena} palabras; la rota que solo caza la \
             longitud, {solo_por_longitud}"
        );

        assert_eq!(mas_corta_buena, 13, "el corpus de campo ha cambiado");
        assert_eq!(solo_por_longitud, 10, "el corpus de campo ha cambiado");
        assert!(MIN_WORDS > solo_por_longitud, "{MIN_WORDS} deja pasar una rota");
        assert!(MIN_WORDS < mas_corta_buena, "{MIN_WORDS} marca la buena mas corta");
    }

    #[test]
    fn la_marca_de_no_habla_se_ve() {
        let review = review("[Música]");
        assert!(review.reasons.contains(&Reason::NonSpeechMarker));
    }

    /// El caso que enseña por que no basta con mirar la caja de la primera letra: whisper
    /// pone mayuscula al empezar aunque lo que oyo empezara a media frase.
    #[test]
    fn una_respuesta_que_empieza_por_conjuncion_con_mayuscula_se_marca() {
        let review = review("Y de último lejos se voy a ir por aquí porque se voy a ir en este quema.");
        assert!(
            review.reasons.contains(&Reason::StartsMidSentence),
            "salio {:?}",
            review.reasons
        );
    }

    #[test]
    fn los_guiones_de_dialogo_se_ven_y_un_guion_de_palabra_no() {
        assert!(review("-¿Qué es lo que? -No me va a dar a vos.")
            .reasons
            .contains(&Reason::DialogueDashes));
        assert!(!review(
            "Trabajé en logística durante el turno de mañana-tarde preparando pedidos con cuidado."
        )
        .reasons
        .contains(&Reason::DialogueDashes));
    }

    /// Los guiones no cuentan como palabras: si contaran, una ristra de ellos disfrazaria de
    /// respuesta larga algo que no lo es.
    #[test]
    fn los_signos_sueltos_no_cuentan_como_palabras() {
        assert_eq!(palabras("- - - hola - - -"), 1);
    }

    #[test]
    fn una_respuesta_vacia_es_sospechosa_y_no_revienta() {
        let review = review("   ");
        assert!(review.suspicious);
        assert_eq!(review.words, 0);
    }

    /// El umbral cae entre los dos corpus, y esto lo fija: la buena mas corta tiene 13
    /// palabras y la mala mas larga de las cazables por longitud tiene 8.
    #[test]
    fn el_umbral_de_palabras_cae_entre_los_dos_corpus() {
        let buena_mas_corta = FRASES
            .iter()
            .map(|(_, texto)| palabras(texto))
            .min()
            .expect("hay frases");

        assert_eq!(buena_mas_corta, 13, "el corpus de referencia ha cambiado");
        assert!(
            MIN_WORDS < buena_mas_corta,
            "{MIN_WORDS} palabras marca la respuesta buena mas corta del corpus"
        );
        assert!(
            MIN_WORDS > palabras("y se un sistema de inventario para empresas."),
            "{MIN_WORDS} palabras no caza la envenenada mas larga de las cortas"
        );
    }
}
