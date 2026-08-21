//! Cuanto se equivoca una transcripcion, y **en que se equivoca**.
//!
//! La medida estandar en reconocimiento de voz es el WER (word error rate): distancia de
//! edicion entre lo que se dijo y lo que se transcribio, contada en palabras y dividida
//! entre las palabras de la referencia.
//!
//! Lo que aqui importa mas que el numero es el **desglose**, y esa es la razon de escribir
//! esto en vez de tirar de una libreria de similitud difusa. Un unico porcentaje de
//! parecido mezcla los tres errores en uno, y los tres fallos medidos el 2026-08-21 son de
//! tipos distintos:
//!
//! | Lo que se guardo | Que error es |
//! |---|---|
//! | "Santiago y tengo 21 años" por "Me llamo Santiago y tengo 21 años" | dos **borrados**, los dos al principio |
//! | "[Música]" sobre un turno de 64 ms | la referencia entera **borrada**, tasa 1,00 |
//! | "¡Aguien es bien!" | **sustituciones** |
//!
//! Y llevan a sitios opuestos. Cero sustituciones con dos borrados dice que el modelo
//! entendio perfectamente lo que le llego, y que el problema esta aguas arriba, en el audio
//! que nunca se le mando: cambiar de modelo no arreglaria nada. Sustituciones altas dicen
//! lo contrario. Un parecido del 85% no distingue esos dos mundos, y elegir mal cuesta el
//! dia.

/// El recuento de errores de una transcripcion frente a lo que de verdad se dijo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Errors {
    /// Palabras que se dijeron y salieron cambiadas.
    pub substitutions: usize,
    /// Palabras que se dijeron y no salieron. Es el sintoma de audio que no llego.
    pub deletions: usize,
    /// Palabras que salieron sin haberse dicho. Es el sintoma de alucinacion.
    pub insertions: usize,
    /// Cuantas palabras tenia la referencia, que es el denominador.
    pub reference_words: usize,
}

impl Errors {
    pub fn total(&self) -> usize {
        self.substitutions + self.deletions + self.insertions
    }

    /// La tasa. Puede pasar de 1 cuando se inventa mas de lo que se dijo, y no se acota:
    /// que un turno de una palabra produzca una parrafada es informacion, no un
    /// desbordamiento.
    ///
    /// Con una referencia vacia el denominador es 1. No es WER —el WER no esta definido
    /// ahi— y tampoco es un caso a medir: una referencia sin palabras es un fallo del
    /// banco de pruebas, no una transcripcion mala.
    pub fn rate(&self) -> f64 {
        self.total() as f64 / self.reference_words.max(1) as f64
    }
}

/// Parte un texto en las palabras con las que se compara.
///
/// Dos indulgencias, y solo dos, por el mismo criterio que la verificacion de citas de §5:
/// **mayusculas y puntuacion no cuentan**. whisper puntua a su manera y penalizar una coma
/// suya seria medir ruido en vez de errores.
///
/// **Los acentos si cuentan**, y no es rigor por gusto: en español "años" y "anos" son dos
/// palabras distintas, y una transcripcion que confunde una con otra se ha equivocado. En
/// cuanto se empiezan a ignorar, "correcto" pasa a significar "aproximadamente".
///
/// Los numeros se dejan como vengan. whisper escribe "21" y no "veintiuno", asi que la
/// referencia hay que escribirla como el la escribiria; convertir unos en otros es una
/// tabla de casos especiales que se equivoca sola y que aqui no hace falta.
pub fn words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|word| {
            word.chars()
                .filter(|c| c.is_alphanumeric())
                .flat_map(char::to_lowercase)
                .collect::<String>()
        })
        .filter(|word| !word.is_empty())
        .collect()
}

/// Compara lo dicho con lo transcrito.
pub fn measure(reference: &str, hypothesis: &str) -> Errors {
    let said = words(reference);
    let heard = words(hypothesis);

    // Distancia de edicion por palabras. `cost[i][j]` es el coste de convertir las primeras
    // `i` palabras dichas en las primeras `j` transcritas.
    let (rows, cols) = (said.len() + 1, heard.len() + 1);
    let mut cost = vec![vec![0usize; cols]; rows];

    for (i, row) in cost.iter_mut().enumerate() {
        row[0] = i; // todo borrado
    }
    for (j, celda) in cost[0].iter_mut().enumerate() {
        *celda = j; // todo inventado
    }

    for i in 1..rows {
        for j in 1..cols {
            let igual = said[i - 1] == heard[j - 1];
            let diagonal = cost[i - 1][j - 1] + usize::from(!igual);
            let borrado = cost[i - 1][j] + 1;
            let insertado = cost[i][j - 1] + 1;
            cost[i][j] = diagonal.min(borrado).min(insertado);
        }
    }

    // El recorrido de vuelta es lo que convierte un numero en un diagnostico: dice de que
    // tipo fue cada error, no solo cuantos hubo.
    let mut out = Errors {
        reference_words: said.len(),
        ..Errors::default()
    };
    let (mut i, mut j) = (said.len(), heard.len());

    while i > 0 || j > 0 {
        // Se prueba la diagonal primero: ante un empate, tratar una palabra como cambiada
        // describe mejor lo que paso que contarla como borrada y ademas inventada.
        if i > 0 && j > 0 {
            let igual = said[i - 1] == heard[j - 1];
            if cost[i][j] == cost[i - 1][j - 1] + usize::from(!igual) {
                if !igual {
                    out.substitutions += 1;
                }
                i -= 1;
                j -= 1;
                continue;
            }
        }
        if i > 0 && cost[i][j] == cost[i - 1][j] + 1 {
            out.deletions += 1;
            i -= 1;
            continue;
        }
        out.insertions += 1;
        j -= 1;
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn una_transcripcion_exacta_no_tiene_errores() {
        let e = measure("me llamo santiago", "me llamo santiago");
        assert_eq!(e.total(), 0);
        assert_eq!(e.rate(), 0.0);
    }

    /// El caso que hay que poder distinguir: falta el principio y **nada mas**.
    ///
    /// Cero sustituciones es el dato que importa. Dice que el modelo entendio bien lo que
    /// recibio, y que lo que falla esta antes de el.
    #[test]
    fn perder_el_principio_son_borrados_y_no_sustituciones() {
        let e = measure(
            "Me llamo Santiago y tengo 21 años",
            "Santiago y tengo 21 años",
        );
        assert_eq!(e.deletions, 2, "{e:?}");
        assert_eq!(e.substitutions, 0, "{e:?}");
        assert_eq!(e.insertions, 0, "{e:?}");
        assert_eq!(e.reference_words, 7);
        assert!((e.rate() - 2.0 / 7.0).abs() < 1e-9);
    }

    /// Una alucinacion corta sobre una frase entera perdida cuesta la frase entera.
    ///
    /// Ojo al reparto, que no es el que parece: la distancia minima no son diez borrados
    /// mas una invencion (once), sino nueve borrados y una sustitucion (diez). El
    /// algoritmo coge el camino barato, que es lo correcto, y aqui se fija para que nadie
    /// lo "arregle" mas adelante creyendo que cuenta mal.
    #[test]
    fn alucinar_sobre_una_frase_perdida_cuesta_la_frase_entera() {
        let e = measure("tuve un conflicto con un compañero del turno de tarde", "[Música]");
        assert_eq!(e.deletions, 9, "{e:?}");
        assert_eq!(e.substitutions, 1, "{e:?}");
        assert_eq!(e.total(), e.reference_words, "{e:?}");
        assert!((e.rate() - 1.0).abs() < 1e-9, "tasa {:.2}: {e:?}", e.rate());
    }

    /// Y la tasa si pasa de 1 cuando se inventa **mas** de lo que se dijo. No se acota:
    /// que un turno de una palabra produzca una parrafada es informacion, no un
    /// desbordamiento.
    #[test]
    fn inventar_mas_de_lo_que_se_dijo_pasa_de_uno() {
        let e = measure("gracias", "muchas gracias por ver el video suscribete al canal");
        assert!(e.rate() > 1.0, "tasa {:.2}: {e:?}", e.rate());
        assert!(e.insertions >= 7, "{e:?}");
    }

    #[test]
    fn una_palabra_cambiada_es_una_sustitucion() {
        let e = measure("preparación diaria de pedidos", "preparación diaria de albaranes");
        assert_eq!(e.substitutions, 1, "{e:?}");
        assert_eq!(e.deletions + e.insertions, 0, "{e:?}");
    }

    #[test]
    fn una_palabra_de_mas_es_una_insercion() {
        let e = measure("control de stock", "control de mucho stock");
        assert_eq!(e.insertions, 1, "{e:?}");
        assert_eq!(e.substitutions + e.deletions, 0, "{e:?}");
    }

    /// whisper puntua a su manera y escribe en mayuscula lo que quiere. Penalizar eso
    /// seria medir su estilo, no sus errores.
    #[test]
    fn ni_las_mayusculas_ni_la_puntuacion_cuentan() {
        let e = measure("carga, descarga y reubicación", "Carga descarga; y reubicación.");
        assert_eq!(e.total(), 0, "{e:?}");
    }

    /// Y el limite de esa indulgencia: en español el acento cambia la palabra.
    #[test]
    fn los_acentos_si_cuentan() {
        let e = measure("cumplí veintiún años", "cumpli veintiun anos");
        assert_eq!(e.substitutions, 3, "{e:?}");
    }

    /// Que no haya salido nada es el caso peor y tiene que medirse, no reventar.
    #[test]
    fn una_transcripcion_vacia_borra_toda_la_referencia() {
        let e = measure("dos palabras aqui", "");
        assert_eq!(e.deletions, 3, "{e:?}");
        assert_eq!(e.rate(), 1.0);
    }

    /// Una referencia vacia no es una medicion: es un fallo del banco. Lo unico que se
    /// exige es que no divida entre cero.
    #[test]
    fn una_referencia_vacia_no_divide_entre_cero() {
        let e = measure("", "algo inventado");
        assert_eq!(e.insertions, 2, "{e:?}");
        assert!(e.rate().is_finite());
    }

    /// El desglose tiene que sumar la distancia de edicion, o el recorrido de vuelta esta
    /// contando otra cosa distinta de lo que la matriz calculo.
    #[test]
    fn el_desglose_cuadra_con_la_distancia() {
        let casos = [
            ("uno dos tres cuatro cinco", "uno tres cuatro seis cinco siete"),
            ("a b c", "x y z"),
            ("preparación de pedidos", "la preparación diaria de los pedidos"),
        ];

        for (dicho, oido) in casos {
            let e = measure(dicho, oido);
            let said = words(dicho).len();
            let heard = words(oido).len();
            // Cada palabra de la referencia se empareja, se cambia o se borra; cada una de
            // la hipotesis se empareja, se cambia o se inventa.
            let emparejadas = said - e.substitutions - e.deletions;
            assert_eq!(
                heard,
                emparejadas + e.substitutions + e.insertions,
                "{dicho:?} -> {oido:?}: {e:?}"
            );
        }
    }
}
