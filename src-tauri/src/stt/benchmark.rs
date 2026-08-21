//! Comparacion de configuraciones de transcripcion, con WER.
//!
//! No es un test de correccion: es la medicion con la que se decide si vale la pena
//! cambiar de modelo o tocar los ajustes del decodificador. Se corre a mano:
//!
//! ```text
//! INTERVIEW_COPILOT_WHISPER=<ggml-base.bin> cargo test --lib -- --ignored --nocapture --test-threads=1 compara_configuraciones
//! ```
//!
//! **Lo que mide y lo que no.** El audio entra directo a whisper desde un WAV, sin pasar
//! por el microfono ni por el VAD. Es deliberado: aisla el modelo de todo lo demas, que es
//! lo unico que permite atribuir una mejora a la configuracion y no al azar de una toma.
//!
//! El precio de ese aislamiento hay que tenerlo presente: **este banco no puede ver el
//! fallo de "falta el principio"**, porque ahi el audio nunca llega a whisper — lo pierde
//! el VAD antes. Un WER bueno aqui no dice que la cadena funcione. Dice que el modelo
//! entiende lo que le llega.
//!
//! Segunda salvedad, y va en la misma direccion: **el sintetizador de Windows habla mas
//! limpio de lo que habla nadie** por un micro de auriculares. Los numeros de aqui sirven
//! para ordenar configuraciones entre si, no para predecir el acierto real sobre la voz de
//! una persona. Para eso hace falta la voz de esa persona.

#![cfg(test)]

use std::path::PathBuf;

use crate::stt::wer::{self, Errors};
use crate::stt::whisper::{LocalWhisper, Tuning};
use crate::stt::SttProvider;
use crate::testing::{self, FRASES};

/// Una configuracion a comparar.
struct Configuracion {
    nombre: &'static str,
    /// Contexto inicial. `Pregunta` usa la pregunta que se esta contestando; `Fijo` usa
    /// siempre el mismo texto y solo existe para el control de abajo.
    prompt: Prompt,
    suprimir_no_habla: bool,
}

enum Prompt {
    Ninguno,
    Pregunta,
    Fijo(&'static str),
}

/// El texto del control.
///
/// whisper copia la ortografia de su contexto inicial, asi que un prompt lleno de faltas
/// deliberadas tiene que ensuciar la salida. **Si no la ensucia, el mando no esta
/// conectado**, y entonces comparar "con pregunta" contra "sin pregunta" no mide nada: las
/// dos serian la misma configuracion con dos nombres. Un banco que nunca ha visto cambiar
/// lo que dice medir no ha demostrado nada.
const PROMPT_DE_CONTROL: &str =
    "PIKING, PAKING, KARRETILLERO, TRANSPALETA ELEKTRIKA, INBENTARIO, LOJISTICA.";

const CONFIGURACIONES: &[Configuracion] = &[
    Configuracion {
        nombre: "base, como esta hoy",
        prompt: Prompt::Ninguno,
        suprimir_no_habla: false,
    },
    Configuracion {
        nombre: "base + suppress_nst",
        prompt: Prompt::Ninguno,
        suprimir_no_habla: true,
    },
    Configuracion {
        nombre: "base + pregunta de contexto",
        prompt: Prompt::Pregunta,
        suprimir_no_habla: false,
    },
    Configuracion {
        nombre: "base + las dos",
        prompt: Prompt::Pregunta,
        suprimir_no_habla: true,
    },
    Configuracion {
        nombre: "CONTROL: prompt con faltas",
        prompt: Prompt::Fijo(PROMPT_DE_CONTROL),
        suprimir_no_habla: false,
    },
];

fn suma(total: &mut Errors, parcial: &Errors) {
    total.substitutions += parcial.substitutions;
    total.deletions += parcial.deletions;
    total.insertions += parcial.insertions;
    total.reference_words += parcial.reference_words;
}

/// **El banco.** Recorre las configuraciones sobre el mismo audio y saca la tabla.
///
/// `INTERVIEW_COPILOT_WHISPER=<ggml-base.bin> cargo test --lib -- --ignored --nocapture --test-threads=1 compara_configuraciones`
#[test]
#[ignore = "sintetiza voz, carga whisper y tarda"]
fn compara_configuraciones_de_transcripcion() {
    let modelo = std::env::var("INTERVIEW_COPILOT_WHISPER").expect("INTERVIEW_COPILOT_WHISPER");
    let dir = testing::banco_dir("stt");

    let muestras = testing::audios_del_corpus(&dir);
    let audio_ms: usize = muestras.iter().map(|m| m.len() * 1000 / 16_000).sum();
    println!(
        "{} frases, {:.1} s de audio sintetizado en {}\n",
        FRASES.len(),
        audio_ms as f64 / 1000.0,
        dir.display()
    );

    let mut resumen: Vec<(&str, Errors, u128)> = Vec::new();
    // Lo transcrito por cada configuracion, para poder comparar el control con la base.
    let mut salidas: Vec<Vec<String>> = Vec::new();

    for config in CONFIGURACIONES {
        let mut whisper =
            LocalWhisper::load(&PathBuf::from(&modelo), "whisper-base").expect("cargar whisper");

        let mut total = Errors::default();
        let mut ms = 0u128;
        let mut dichas: Vec<String> = Vec::new();

        println!("=== {} ===", config.nombre);
        for ((pregunta, dicho), audio) in FRASES.iter().zip(&muestras) {
            whisper.tune(Tuning {
                initial_prompt: match config.prompt {
                    Prompt::Ninguno => None,
                    Prompt::Pregunta => Some((*pregunta).to_owned()),
                    Prompt::Fijo(texto) => Some(texto.to_owned()),
                },
                suppress_non_speech: config.suprimir_no_habla,
            });

            let empezo = std::time::Instant::now();
            let oido = whisper.transcribe(audio, Some("es")).expect("transcribir");
            ms += empezo.elapsed().as_millis();

            let errores = wer::measure(dicho, &oido);
            suma(&mut total, &errores);
            dichas.push(oido.clone());

            println!(
                "  WER {:.3}  (S{} B{} I{})  {}",
                errores.rate(),
                errores.substitutions,
                errores.deletions,
                errores.insertions,
                oido
            );
        }

        println!(
            "  --> WER {:.3} · {} sustituciones, {} borrados, {} inserciones · {} ms ({:.2}x tiempo real)\n",
            total.rate(),
            total.substitutions,
            total.deletions,
            total.insertions,
            ms,
            ms as f64 / audio_ms as f64
        );
        resumen.push((config.nombre, total, ms));
        salidas.push(dichas);
    }

    println!(
        "{:<32} {:>7} {:>5} {:>5} {:>5} {:>9}",
        "configuracion", "WER", "S", "B", "I", "x real"
    );
    for (nombre, errores, ms) in &resumen {
        println!(
            "{nombre:<32} {:>7.3} {:>5} {:>5} {:>5} {:>9.2}",
            errores.rate(),
            errores.substitutions,
            errores.deletions,
            errores.insertions,
            *ms as f64 / audio_ms as f64
        );
    }

    // Sin asercion sobre cual gana: eso es la conclusion que este banco existe para sacar,
    // y fijarla antes de mirarla seria escribirla a mano. Lo unico que se exige es que la
    // linea base no este rota, porque entonces la comparacion no mide nada.
    let (_, base, _) = resumen.first().expect("hay configuraciones");
    assert!(
        base.rate() < 1.0,
        "la configuracion de hoy falla mas palabras que la referencia entera ({:.3}): \
         el banco no esta midiendo lo que cree",
        base.rate()
    );

    // Y el control. Sin esto, "el prompt no cambia nada" y "el prompt no llega" se leen
    // igual en la tabla, y son conclusiones opuestas.
    let primera = salidas.first().expect("hay salidas");
    let control = salidas.last().expect("hay control");
    assert_ne!(
        primera, control,
        "el prompt con faltas dio exactamente lo mismo que no poner ninguno: el contexto
         inicial no esta llegando al decodificador, asi que la fila 'con pregunta' de la
         tabla no mide lo que dice"
    );
}
