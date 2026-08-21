//! Banco del arranque de turno: cuanto del principio se pierde, y cuanto dura un turno corto.
//!
//! Es el banco que le faltaba al de §4.4. Aquel mete el audio directo a whisper y por eso
//! **no puede ver** el fallo de "falta el principio": ahi el audio nunca llega al modelo,
//! lo tira el VAD antes. Este mide justo ese trozo de la cadena, y no toca whisper.
//!
//! Se corre a mano:
//!
//! ```text
//! INTERVIEW_COPILOT_VAD=<silero_vad.onnx> cargo test --lib -- --ignored --nocapture --test-threads=1 arranque_de_turno
//! INTERVIEW_COPILOT_VAD=<silero_vad.onnx> cargo test --lib -- --ignored --nocapture --test-threads=1 turno_corto
//! ```
//!
//! **Las dos preguntas que contesta.**
//!
//! 1. *Cuanto colchon hace falta para no comerse el principio.* Se compara el instante en
//!    que la senal empieza de verdad —medido por energia, no por el VAD— con el instante en
//!    que `TurnDetector` abre turno. La diferencia es el colchon que haria falta para no
//!    perder nada; `PREROLL_FRAMES` son hoy 256 ms y esta anotado como "de sobra", que es
//!    una suposicion de 2026-08-19 que nadie midio.
//! 2. *Cuanto dura el turno corto mas corto que si cuenta.* El turno espurio que produjo
//!    `[Música]` el 2026-08-21 duro 64 ms, que son exactamente las dos ventanas que exige
//!    `FRAMES_TO_START`. Para poder tirarlo hace falta saber donde esta el suelo de un "si"
//!    legitimo, porque tirar tambien los "si" seria cambiar un fallo por otro.
//!
//! **La ganancia es la columna que importa.** El mismo audio se pasa a volumen entero y
//! rebajado. No es un capricho: la distancia entre el 0,089 de WER sobre voz limpia y lo que
//! salio de la voz real esta en el camino del audio, y "el microfono entra bajo" es la
//! primera hipotesis. Si el colchon necesario crece al bajar la ganancia, un microfono flojo
//! **es** un principio comido, y entonces subirlo en Windows arregla parte del problema
//! antes de tocar ninguna constante.
//!
//! **Y los controles**, porque un banco sin control no distingue "no cambia nada" de "no
//! esta conectado":
//!
//! - *Silencio digital delante.* Un segundo de ceros antes de la frase. El instante en que
//!   abre el turno tiene que correrse un segundo entero y el **colchon necesario no debe
//!   moverse**. Si se moviera, esta tabla estaria midiendo el relleno y no el arranque.
//! - *El detector de energia no mira el volumen.* El instante en que empieza la senal se
//!   mide relativo al pico del propio fichero, asi que tiene que salir **identico** a las
//!   cuatro ganancias. Si cambiara, la columna del colchon estaria midiendo el detector de
//!   energia en vez del VAD, que es justo la conclusion contraria.
//! - *Solo silencio.* Un fichero de ceros no puede abrir ningun turno. Si lo abriera, todas
//!   las filas de arriba estarian de sobra.
//!
//! **Lo que este banco no puede ver**, y hay que decirlo igual que en §4.4. Lo primero, el
//! suelo de ruido: bajar la ganancia de un fichero limpio baja tambien su ruido, y un
//! microfono flojo de verdad no hace eso —la voz baja y el ruido de sala se queda donde
//! estaba—. La fila de la decima parte descarta que el volumen por si solo retrase el
//! arranque; no descarta que una relacion senal-ruido mala lo haga. Y lo segundo, el
//! transitorio de apertura del microfono. Lo produce el dispositivo al abrirse, no el
//! fichero, asi que aqui no existe. Del transitorio real hay una sola observacion, la del 2026-08-21, y
//! una observacion no es una distribucion. La segunda mitad de la fila la tiene que traer el
//! volcado de turnos cortos desde la app.

#![cfg(test)]

use std::path::Path;

use crate::audio::resample::TARGET_HZ;
use crate::audio::vad::{Event, VoiceTracker, FRAME_MS, FRAME_SAMPLES, PREROLL_FRAMES};
use crate::testing;

/// Ventana con la que se busca el principio de la senal. Cinco milisegundos es mucho mas
/// fino que las ventanas de 32 ms del VAD, que es lo que hace falta para que el numero que
/// sale sea el arranque y no el redondeo del propio banco.
const VENTANA_ENERGIA: usize = TARGET_HZ as usize * 5 / 1000;

/// Fraccion del pico de energia del fichero a partir de la cual se considera que la senal ha
/// empezado. Es **relativa al propio fichero** a proposito: asi el numero no depende del
/// volumen, que es justo la variable que este banco mueve.
const FRACCION_DE_ARRANQUE: f32 = 0.05;

/// Una condicion a comparar sobre el mismo audio.
struct Condicion {
    nombre: &'static str,
    ganancia: f32,
    /// Milisegundos de ceros por delante.
    silencio_ms: usize,
}

const CONDICIONES: &[Condicion] = &[
    Condicion {
        nombre: "volumen entero",
        ganancia: 1.0,
        silencio_ms: 0,
    },
    Condicion {
        nombre: "a la mitad",
        ganancia: 0.50,
        silencio_ms: 0,
    },
    Condicion {
        nombre: "a la cuarta parte",
        ganancia: 0.25,
        silencio_ms: 0,
    },
    Condicion {
        nombre: "a la decima parte",
        ganancia: 0.10,
        silencio_ms: 0,
    },
    Condicion {
        nombre: "CONTROL: 1 s de silencio delante",
        ganancia: 1.0,
        silencio_ms: 1000,
    },
];

/// Lo que sale de pasar un audio por el VAD.
#[derive(Debug, Clone, Copy)]
struct Medida {
    /// Donde empieza la senal de verdad, por energia.
    arranque_ms: usize,
    /// Donde `TurnDetector` da el turno por empezado.
    abre_ms: Option<usize>,
    probabilidad_maxima: f32,
}

impl Medida {
    /// Colchon que haria falta para no perder ni una muestra del principio.
    fn colchon_necesario_ms(&self) -> Option<usize> {
        self.abre_ms
            .map(|abre| abre.saturating_sub(self.arranque_ms))
    }

    /// Lo que se tira hoy, con el colchon que hay puesto.
    fn perdido_hoy_ms(&self) -> Option<usize> {
        self.colchon_necesario_ms()
            .map(|colchon| colchon.saturating_sub(PREROLL_FRAMES * FRAME_MS))
    }
}

/// El instante en que la senal empieza, medido por energia y no por el VAD.
///
/// Tiene que ser independiente del VAD: si el principio se buscara con el propio detector
/// que se esta midiendo, la columna del colchon saldria siempre cero y la tabla no diria
/// nada.
fn arranque_por_energia(audio: &[f32]) -> usize {
    let energias: Vec<f32> = audio
        .chunks(VENTANA_ENERGIA)
        .map(|ventana| {
            let suma: f32 = ventana.iter().map(|muestra| muestra * muestra).sum();
            (suma / ventana.len() as f32).sqrt()
        })
        .collect();

    let pico = energias.iter().fold(0.0f32, |max, e| max.max(*e));
    let umbral = pico * FRACCION_DE_ARRANQUE;

    energias
        .iter()
        .position(|energia| *energia >= umbral)
        .unwrap_or(0)
        * VENTANA_ENERGIA
        * 1000
        / TARGET_HZ as usize
}

/// Prepara una variante del audio: ceros por delante y ganancia.
fn variante(audio: &[f32], condicion: &Condicion) -> Vec<f32> {
    let ceros = TARGET_HZ as usize * condicion.silencio_ms / 1000;
    let mut salida = vec![0.0f32; ceros];
    salida.extend(audio.iter().map(|muestra| muestra * condicion.ganancia));
    salida
}

/// Pasa un audio entero por el VAD, con un modelo recien cargado.
///
/// Recien cargado en cada pasada porque Silero es recurrente: reutilizar el tracker entre
/// condiciones haria que la segunda arrancase con el estado que dejo la primera, y entonces
/// la tabla mediria el orden de las filas.
fn mide(modelo: &Path, audio: &[f32]) -> Medida {
    let mut tracker = VoiceTracker::new(modelo).expect("cargar el modelo del VAD");
    let mut abre_ms = None;
    let mut probabilidad_maxima = 0.0f32;

    for (indice, ventana) in audio.chunks_exact(FRAME_SAMPLES).enumerate() {
        let evento = tracker.push(ventana).expect("inferir");
        probabilidad_maxima = probabilidad_maxima.max(tracker.probability());
        if matches!(evento, Event::SpeechStarted) && abre_ms.is_none() {
            abre_ms = Some(indice * FRAME_MS);
        }
    }

    Medida {
        arranque_ms: arranque_por_energia(audio),
        abre_ms,
        probabilidad_maxima,
    }
}

fn ms(valor: Option<usize>) -> String {
    valor.map_or_else(|| "no abre".to_owned(), |v| format!("{v}"))
}

/// **El banco del arranque.** Cuanto del principio se pierde, y como cambia con el volumen.
///
/// `INTERVIEW_COPILOT_VAD=<onnx> cargo test --lib -- --ignored --nocapture --test-threads=1 arranque_de_turno`
#[test]
#[ignore = "sintetiza voz, carga el modelo del VAD y tarda"]
fn cuanto_del_arranque_se_pierde() {
    let modelo = std::env::var("INTERVIEW_COPILOT_VAD").expect("INTERVIEW_COPILOT_VAD");
    let modelo = Path::new(&modelo);
    let dir = testing::banco_dir("arranque");
    let audios = testing::audios_del_corpus(&dir);

    println!(
        "{} frases sintetizadas en {}\ncolchon puesto hoy: PREROLL_FRAMES = {} ventanas = {} ms\n",
        audios.len(),
        dir.display(),
        PREROLL_FRAMES,
        PREROLL_FRAMES * FRAME_MS
    );

    // Por condicion: el colchon que habria hecho falta en cada frase.
    let mut colchones: Vec<(&str, Vec<Option<usize>>)> = Vec::new();
    // Por condicion: donde dijo el detector de energia que empieza la senal.
    let mut arranques: Vec<(&str, Vec<usize>)> = Vec::new();

    for condicion in CONDICIONES {
        println!("=== {} ===", condicion.nombre);
        println!(
            "  {:<8} {:>10} {:>10} {:>12} {:>12} {:>8}",
            "frase", "senal ms", "abre ms", "colchon ms", "perdido ms", "p max"
        );

        let mut de_esta = Vec::new();
        let mut arranques_de_esta = Vec::new();

        for (indice, audio) in audios.iter().enumerate() {
            let medida = mide(modelo, &variante(audio, condicion));
            println!(
                "  {:<8} {:>10} {:>10} {:>12} {:>12} {:>8.3}",
                indice,
                medida.arranque_ms,
                ms(medida.abre_ms),
                ms(medida.colchon_necesario_ms()),
                ms(medida.perdido_hoy_ms()),
                medida.probabilidad_maxima
            );
            de_esta.push(medida.colchon_necesario_ms());
            arranques_de_esta.push(medida.arranque_ms);
        }

        colchones.push((condicion.nombre, de_esta));
        arranques.push((condicion.nombre, arranques_de_esta));
        println!();
    }

    println!(
        "{:<36} {:>8} {:>12} {:>12} {:>12}",
        "condicion", "abre", "colchon max", "colchon medio", "perdido max"
    );
    for (nombre, medidos) in &colchones {
        let abiertos: Vec<usize> = medidos.iter().filter_map(|c| *c).collect();
        if abiertos.is_empty() {
            println!(
                "{nombre:<36} {:>8} {:>12} {:>12} {:>12}",
                "0", "-", "-", "-"
            );
            continue;
        }
        let maximo = *abiertos.iter().max().expect("hay medidas");
        let medio = abiertos.iter().sum::<usize>() / abiertos.len();
        println!(
            "{nombre:<36} {:>8} {:>12} {:>12} {:>12}",
            format!("{}/{}", abiertos.len(), medidos.len()),
            maximo,
            medio,
            maximo.saturating_sub(PREROLL_FRAMES * FRAME_MS)
        );
    }

    // Nada de aserciones sobre cual gana: esa es la conclusion que el banco existe para
    // sacar. Lo que se exige es que el banco este midiendo lo que dice.

    let (_, base) = colchones.first().expect("hay condiciones");
    assert!(
        base.iter().all(Option::is_some),
        "a volumen entero alguna frase no abrio turno: el banco no esta midiendo nada"
    );

    // Control 1: el relleno corre el instante en que abre, no el colchon necesario.
    let (_, control) = colchones.last().expect("hay control");
    for (indice, (sin_relleno, con_relleno)) in base.iter().zip(control).enumerate() {
        let (Some(sin), Some(con)) = (sin_relleno, con_relleno) else {
            panic!("la frase {indice} no abrio turno en una de las dos condiciones");
        };
        let diferencia = sin.abs_diff(*con);
        assert!(
            diferencia <= FRAME_MS,
            "la frase {indice} necesito {sin} ms de colchon sin relleno y {con} ms con un \
             segundo de ceros delante: el banco esta midiendo el relleno, no el arranque"
        );
    }

    // Control 2: el detector de energia no mira el volumen. Si el arranque se moviera con
    // la ganancia, la columna del colchon estaria midiendo el detector y no el VAD.
    let (_, arranque_base) = arranques.first().expect("hay condiciones");
    for (nombre, medidos) in arranques.iter().skip(1) {
        if nombre.starts_with("CONTROL") {
            continue;
        }
        assert_eq!(
            arranque_base, medidos,
            "con la ganancia de '{nombre}' el arranque por energia salio en otro sitio: \
             el detector de energia depende del volumen y la tabla no mide el VAD"
        );
    }

    // Control 3: solo ceros no puede abrir un turno.
    let solo_silencio = vec![0.0f32; TARGET_HZ as usize * 3];
    let medida = mide(modelo, &solo_silencio);
    assert!(
        medida.abre_ms.is_none(),
        "tres segundos de ceros abrieron un turno (p max {:.3}): con eso, ninguna fila de \
         arriba significa nada",
        medida.probabilidad_maxima
    );
}

/// **El banco del turno corto.** Cuanto dura el turno legitimo mas corto.
///
/// El numero que sale es el suelo por debajo del cual se puede tirar un turno sin tirar una
/// respuesta. El techo lo pone la unica observacion que hay del transitorio de apertura: 64
/// ms el 2026-08-21. Si el suelo de aqui no queda holgadamente por encima de esos 64 ms, no
/// hay ninguna duracion minima que separe las dos cosas y el fallo (b) hay que atacarlo por
/// otro lado.
///
/// `INTERVIEW_COPILOT_VAD=<onnx> cargo test --lib -- --ignored --nocapture --test-threads=1 turno_corto`
#[test]
#[ignore = "sintetiza voz, carga el modelo del VAD y tarda"]
fn cuanto_dura_un_turno_corto_legitimo() {
    let modelo = std::env::var("INTERVIEW_COPILOT_VAD").expect("INTERVIEW_COPILOT_VAD");
    let modelo = Path::new(&modelo);
    let dir = testing::banco_dir("turno-corto");

    println!(
        "{:<12} {:>10} {:>12} {:>10}",
        "palabra", "audio ms", "voz del turno", "p max"
    );

    let mut duraciones = Vec::new();
    for (indice, palabra) in testing::PALABRAS_CORTAS.iter().enumerate() {
        let wav = dir.join(format!("corta{indice}.wav"));
        testing::sintetiza(palabra, &wav);
        let audio = testing::lee_wav_16k(&wav);

        // Con silencio detras, que es lo que cierra el turno y da la duracion de voz.
        let mut con_cola = audio.clone();
        con_cola.extend(std::iter::repeat_n(0.0f32, TARGET_HZ as usize));

        let mut tracker = VoiceTracker::new(modelo).expect("cargar el modelo del VAD");
        let mut voz_ms = None;
        let mut maxima = 0.0f32;
        for ventana in con_cola.chunks_exact(FRAME_SAMPLES) {
            let evento = tracker.push(ventana).expect("inferir");
            maxima = maxima.max(tracker.probability());
            if let Event::TurnEnded { speech_ms } = evento {
                voz_ms.get_or_insert(speech_ms);
            }
        }

        println!(
            "{:<12} {:>10} {:>12} {:>10.3}",
            palabra,
            audio.len() * 1000 / TARGET_HZ as usize,
            ms(voz_ms),
            maxima
        );
        if let Some(voz) = voz_ms {
            duraciones.push(voz);
        }
    }

    assert!(
        !duraciones.is_empty(),
        "ninguna palabra corta cerro un turno: el banco no esta midiendo nada"
    );

    let minimo = *duraciones.iter().min().expect("hay duraciones");
    println!(
        "\nel turno legitimo mas corto duro {minimo} ms de voz.\n\
         el transitorio observado el 2026-08-21 duro 64 ms ({} ventanas, justo las que \
         exige FRAMES_TO_START).\n\
         margen entre los dos: {} ms",
        64 / FRAME_MS,
        minimo as i64 - 64
    );
}
