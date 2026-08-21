//! Instrumental compartido por los bancos de medida. No es codigo de la aplicacion.
//!
//! Existe porque el sintetizador de Windows y el lector de WAV llegaron a estar escritos
//! tres veces —en el banco de transcripcion, en los tests del VAD y otra vez en el banco
//! del arranque de turno—, y tres copias de un instrumento de medida son tres instrumentos
//! distintos en cuanto una de ellas se toca.
//!
//! Que el corpus de frases sea **el mismo** para todos los bancos tampoco es comodidad: el
//! WER de §4.4 y el colchon que mide `audio::benchmark` hablan de la misma cadena rota por
//! sitios distintos, y comparar dos numeros sacados de audio distinto no diria nada.

#![cfg(test)]

use std::path::{Path, PathBuf};

/// Lo que se le hace decir al sintetizador, con la pregunta que lo motiva.
///
/// Son respuestas del dominio del CV real —almacen y logistica— y no frases de laboratorio:
/// el vocabulario es la mitad del problema, y un banco con "el rapido zorro marron" mediria
/// otra cosa. Las dos primeras empiezan como las dos que se guardaron cortadas el
/// 2026-08-21, "Me llamo…" y "Diseñé…".
pub const FRASES: &[(&str, &str)] = &[
    (
        "Cuéntame un poco sobre ti",
        "Me llamo Santiago Urbaneja y llevo casi tres años trabajando en logística y almacén.",
    ),
    (
        "Cuéntame un proyecto complicado en el que hayas trabajado",
        "Diseñé un sistema de inventario para llevar el control de stock en una hoja de Excel.",
    ),
    (
        "Háblame de una vez que tuviste un conflicto con un compañero",
        "Tuve un conflicto con un compañero del turno de tarde que dejaba la zona de picking sin recoger.",
    ),
    (
        "¿Cómo es un día normal en tu trabajo actual o en el último?",
        "Preparo los pedidos con picking y packing, cumpliendo estrictamente los tiempos de entrega.",
    ),
    (
        "¿Con qué herramientas o programas sueles trabajar?",
        "Tengo el carnet de carretillero en vigor y manejo la transpaleta eléctrica a diario.",
    ),
    (
        "¿Qué haces cuando no sabes cómo resolver algo?",
        "Cuando no sé cómo resolver algo, pregunto al encargado antes de improvisar por mi cuenta.",
    ),
];

/// Respuestas legitimas de un turno corto.
///
/// Son la otra mitad del problema del turno espurio: para poder tirar un turno de 64 ms hay
/// que saber cuanto dura el turno corto **mas corto que si cuenta**. Un "si" y un "vale" son
/// lo que contesta cualquiera cuando el entrevistador pregunta si se le oye bien, y tirarlos
/// seria cambiar un fallo por otro.
pub const PALABRAS_CORTAS: &[&str] = &["Sí.", "No.", "Ya.", "Vale.", "Correcto.", "Ajá."];

/// Carpeta de trabajo de un banco, dentro del temporal del sistema.
///
/// Los WAV se generan una vez y se reutilizan entre ejecuciones: sintetizar de nuevo en
/// cada pasada mediria el sintetizador, que varia entre tomas, en vez de lo que se compara.
pub fn banco_dir(nombre: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("interview-copilot-banco-{nombre}"));
    std::fs::create_dir_all(&dir).expect("crear el directorio del banco");
    dir
}

/// Hace hablar al sintetizador de Windows y deja el WAV en disco, si no estaba ya.
///
/// El texto viaja por fichero y no por la linea de comandos: los acentos se pierden al
/// cruzar la pagina de codigos de la consola, y un banco que mide acentos no puede
/// permitirse estropearlos por el camino.
pub fn sintetiza(texto: &str, destino: &Path) {
    if destino.is_file() {
        return;
    }

    let fuente = destino.with_extension("txt");
    // Con marca de orden de bytes, que es lo que hace que .NET lo lea como UTF-8.
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice(texto.as_bytes());
    std::fs::write(&fuente, bytes).expect("escribir el texto a decir");

    // 16 kHz mono, que es justo lo que comen whisper y Silero: pedirselo al sintetizador
    // evita un remuestreo que no tiene por que estar en medio de una medicion.
    let script = format!(
        "Add-Type -AssemblyName System.Speech; \
         $t = [IO.File]::ReadAllText('{}'); \
         $v = New-Object System.Speech.Synthesis.SpeechSynthesizer; \
         $f = New-Object System.Speech.AudioFormat.SpeechAudioFormatInfo(16000, \
              [System.Speech.AudioFormat.AudioBitsPerSample]::Sixteen, \
              [System.Speech.AudioFormat.AudioChannel]::Mono); \
         $v.SetOutputToWaveFile('{}', $f); \
         $v.Speak($t); \
         $v.Dispose()",
        fuente.display(),
        destino.display()
    );

    let estado = std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .status()
        .expect("lanzar el sintetizador");
    assert!(estado.success(), "el sintetizador fallo: {estado:?}");
    assert!(destino.is_file(), "el sintetizador no dejo el wav");
}

/// Lee un WAV PCM de 16 bits y lo deja en 16 kHz mono, por el mismo camino que el audio en
/// vivo: si el remuestreo estropea la senal, quien lo use se entera.
pub fn lee_wav_16k(path: &Path) -> Vec<f32> {
    let bytes = std::fs::read(path).expect("leer el wav");
    let canales = u16::from_le_bytes([bytes[22], bytes[23]]);
    let frecuencia = u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]);
    let data = bytes
        .windows(4)
        .position(|w| w == b"data")
        .map(|pos| pos + 8)
        .expect("el wav no trae bloque data");

    let crudas: Vec<f32> = bytes[data..]
        .chunks_exact(2)
        .map(|par| i16::from_le_bytes([par[0], par[1]]) as f32 / 32768.0)
        .collect();

    let mut muestras = Vec::new();
    crate::audio::resample::to_mono_16k(&crudas, canales, frecuencia, &mut muestras);
    muestras
}

/// Sintetiza el corpus de `FRASES` una sola vez y lo devuelve ya en 16 kHz mono.
pub fn audios_del_corpus(dir: &Path) -> Vec<Vec<f32>> {
    FRASES
        .iter()
        .enumerate()
        .map(|(indice, (_, texto))| {
            let wav = dir.join(format!("frase{indice}.wav"));
            sintetiza(texto, &wav);
            lee_wav_16k(&wav)
        })
        .collect()
}
