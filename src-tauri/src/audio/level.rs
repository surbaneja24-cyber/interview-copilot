//! Medidor de nivel: la parte del audio que se puede probar sin micrófono.
//!
//! Aqui no hay cpal ni hilos, solo aritmetica sobre muestras. Es a proposito: el resto
//! del modulo solo se puede comprobar hablando, y lo que se comprueba hablando no tiene
//! test de regresion.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Instant;

/// Suelo del medidor en decibelios a fondo de escala.
///
/// El silencio digital absoluto son −∞ dB, y **eso no se puede mandar a la UI**: JSON no
/// tiene infinito, asi que `serde_json` lo serializa como `null` y al otro lado hay un
/// `number`. Un suelo finito ademas es lo que quiere una barra: por debajo de −60 dB no
/// hay nada que ensenar y se necesita un extremo para la escala.
pub const FLOOR_DBFS: f32 = -100.0;

/// Nivel de una ventana de audio, en decibelios a fondo de escala (0 dB = maximo).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Level {
    /// Energia media: es lo que se parece a "cuanto suena".
    pub rms_dbfs: f32,
    /// Muestra mas alta: es lo que avisa de saturacion aunque la media sea baja.
    pub peak_dbfs: f32,
}

impl Level {
    pub const SILENT: Self = Self {
        rms_dbfs: FLOOR_DBFS,
        peak_dbfs: FLOOR_DBFS,
    };
}

/// Convierte una amplitud lineal (0..1) a decibelios, con suelo.
pub fn to_dbfs(amplitude: f32) -> f32 {
    if amplitude <= 0.0 {
        return FLOOR_DBFS;
    }
    (20.0 * amplitude.log10()).max(FLOOR_DBFS)
}

/// Mide una ventana de muestras ya normalizadas a −1,0..1,0.
pub fn analyze(samples: &[f32]) -> Level {
    if samples.is_empty() {
        return Level::SILENT;
    }

    let mut sum_squares = 0.0f64;
    let mut peak = 0.0f32;

    for sample in samples {
        let magnitude = sample.abs();
        if magnitude > peak {
            peak = magnitude;
        }
        sum_squares += f64::from(*sample) * f64::from(*sample);
    }

    #[allow(clippy::cast_possible_truncation)]
    let rms = (sum_squares / samples.len() as f64).sqrt() as f32;

    Level {
        rms_dbfs: to_dbfs(rms),
        peak_dbfs: to_dbfs(peak),
    }
}

/// Lo que el hilo de audio escribe y la UI lee.
///
/// Va con atomicos y no con un mutex porque quien escribe es la llamada de retorno de
/// cpal: bloquearse ahi no retrasa un dibujo, corta el audio. Guardar un `f32` en un
/// `AtomicU32` por sus bits es feo y es la forma estandar de hacerlo.
#[derive(Debug)]
pub struct Meter {
    rms: AtomicU32,
    peak: AtomicU32,
    frames: AtomicU64,
    /// Cuando se pidio la captura. El medidor se construye en la primera linea de
    /// `Recorder::start`, asi que este es el instante en que el usuario dio al boton.
    started: Instant,
    /// Microsegundos desde `started` hasta que el dispositivo dijo estar abierto.
    /// Cero significa "todavia no".
    opened_us: AtomicU64,
    /// Microsegundos desde `started` hasta la primera ventana de audio que llego de
    /// verdad. Cero significa "todavia no ha llegado ninguna".
    ///
    /// Es la unica de las dos que dice la verdad sobre lo que se pierde: un dispositivo
    /// puede dar por abierta la sesion y tardar despues en entregar la primera muestra, y
    /// lo que se hable en ese hueco no es que se descarte, es que no existe.
    first_frame_us: AtomicU64,
}

/// Un medidor recien creado esta en el suelo, no en cero. Cero bits es 0,0 dB, que es el
/// **maximo** de la escala: con `derive(Default)` la barra arrancaria a tope.
impl Default for Meter {
    fn default() -> Self {
        Self {
            rms: AtomicU32::new(FLOOR_DBFS.to_bits()),
            peak: AtomicU32::new(FLOOR_DBFS.to_bits()),
            frames: AtomicU64::new(0),
            started: Instant::now(),
            opened_us: AtomicU64::new(0),
            first_frame_us: AtomicU64::new(0),
        }
    }
}

impl Meter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registra una ventana. La llama el hilo de audio, cientos de veces por segundo.
    pub fn push(&self, samples: &[f32]) {
        let level = analyze(samples);
        self.rms.store(level.rms_dbfs.to_bits(), Ordering::Relaxed);

        // El pico se **mantiene** hasta que alguien lo lee. La UI mira cada 100 ms y una
        // ventana de audio dura 10: sin retencion, nueve de cada diez picos no se verian
        // y una saturacion breve pasaria desapercibida, que es justo lo que un medidor
        // tiene que ensenar.
        let stored = f32::from_bits(self.peak.load(Ordering::Relaxed));
        if level.peak_dbfs > stored {
            self.peak.store(level.peak_dbfs.to_bits(), Ordering::Relaxed);
        }

        self.frames
            .fetch_add(samples.len() as u64, Ordering::Relaxed);

        // Al final y no al principio: quien lee el estado vera siempre la marca **y** las
        // muestras contadas. Al reves hay una ventana en la que el estado dice "llego la
        // primera muestra" con el contador todavia a cero, que es justo la contradiccion
        // que hace desconfiar de un diagnostico. Sellar cuesta una carga relajada por
        // llamada; el reloj se consulta una sola vez en toda la captura.
        if self.first_frame_us.load(Ordering::Relaxed) == 0 {
            self.first_frame_us
                .store(self.micros_desde_el_arranque(), Ordering::Relaxed);
        }
    }

    /// Lee el nivel y reinicia el pico retenido.
    pub fn read(&self) -> Level {
        let rms = f32::from_bits(self.rms.load(Ordering::Relaxed));
        let peak = f32::from_bits(
            self.peak
                .swap(FLOOR_DBFS.to_bits(), Ordering::Relaxed),
        );

        Level {
            rms_dbfs: rms,
            peak_dbfs: peak,
        }
    }

    /// Muestras recibidas desde que arranco la captura. Distingue "silencio" de "no llega
    /// nada", que en la pantalla se parecen y no son lo mismo.
    pub fn frames(&self) -> u64 {
        self.frames.load(Ordering::Relaxed)
    }

    /// Anota que el dispositivo ya esta abierto y arrancado. La llama el hilo de captura
    /// una vez, justo despues de `play()`.
    pub fn mark_opened(&self) {
        self.opened_us
            .store(self.micros_desde_el_arranque(), Ordering::Relaxed);
    }

    /// Lo que tardo el dispositivo en decir que estaba abierto, en milisegundos.
    pub fn opened_ms(&self) -> Option<u64> {
        milis(self.opened_us.load(Ordering::Relaxed))
    }

    /// Lo que tardo en llegar la primera muestra, en milisegundos.
    ///
    /// `None` mientras no haya llegado ninguna, que no es lo mismo que cero: es la
    /// diferencia entre "instantaneo" y "el dispositivo abrio y no entrega nada".
    pub fn first_sample_ms(&self) -> Option<u64> {
        milis(self.first_frame_us.load(Ordering::Relaxed))
    }

    /// Nunca devuelve cero: cero es el valor que significa "todavia no", y un arranque
    /// mas rapido que el reloj se leeria como que no ha pasado.
    fn micros_desde_el_arranque(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_micros())
            .unwrap_or(u64::MAX)
            .max(1)
    }
}

/// Microsegundos a milisegundos, con el cero como "todavia no".
fn milis(micros: u64) -> Option<u64> {
    (micros > 0).then_some(micros / 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Un `f32` no puede compararse por igualdad; margen de una centesima de decibelio.
    fn cerca(left: f32, right: f32) -> bool {
        (left - right).abs() < 0.01
    }

    #[test]
    fn el_silencio_llega_al_suelo_y_no_a_menos_infinito() {
        let level = analyze(&[0.0; 128]);
        assert_eq!(level.rms_dbfs, FLOOR_DBFS);
        assert!(
            level.rms_dbfs.is_finite(),
            "un −∞ se serializa como null en JSON y al otro lado hay un number"
        );
    }

    #[test]
    fn la_escala_completa_son_cero_decibelios() {
        assert!(cerca(to_dbfs(1.0), 0.0));
        assert!(cerca(to_dbfs(0.5), -6.02));
        assert!(cerca(to_dbfs(0.1), -20.0));
    }

    /// Un tono puro tiene un RMS 3 dB por debajo de su pico. Si esto falla, es que se
    /// esta midiendo la media de las magnitudes y no la energia.
    #[test]
    fn un_tono_tiene_el_rms_tres_decibelios_bajo_el_pico() {
        let muestras: Vec<f32> = (0..48_000)
            .map(|index| {
                let phase = std::f32::consts::TAU * 440.0 * index as f32 / 48_000.0;
                phase.sin()
            })
            .collect();

        let level = analyze(&muestras);
        assert!(cerca(level.peak_dbfs, 0.0), "pico {}", level.peak_dbfs);
        assert!(
            (level.rms_dbfs + 3.01).abs() < 0.05,
            "rms {} deberia rondar −3,01 dB",
            level.rms_dbfs
        );
    }

    #[test]
    fn una_ventana_vacia_no_revienta() {
        assert_eq!(analyze(&[]), Level::SILENT);
    }

    /// El pico se retiene entre lecturas: un chasquido corto entre dos consultas de la UI
    /// tiene que verse.
    #[test]
    fn el_pico_se_retiene_hasta_que_se_lee() {
        let meter = Meter::new();
        meter.push(&[0.9, 0.0, 0.0, 0.0]);
        meter.push(&[0.001; 4]);

        let primera = meter.read();
        assert!(cerca(primera.peak_dbfs, to_dbfs(0.9)));
        assert!(
            primera.rms_dbfs < -50.0,
            "el rms es el de la ultima ventana, no el del pico: {}",
            primera.rms_dbfs
        );

        // Y despues de leerlo, se reinicia: si no, la barra se quedaria clavada arriba.
        meter.push(&[0.001; 4]);
        assert!(meter.read().peak_dbfs < -50.0);
    }

    /// Sin ventanas no hay primera ventana. Es la distincion que hace util el numero:
    /// "todavia no ha llegado nada" y "llego al instante" no pueden leerse igual.
    #[test]
    fn sin_muestras_no_hay_instante_de_la_primera() {
        let meter = Meter::new();
        assert_eq!(meter.first_sample_ms(), None);
        assert_eq!(meter.opened_ms(), None);

        meter.push(&[0.0; 480]);
        assert!(
            meter.first_sample_ms().is_some(),
            "una ventana de silencio sigue siendo una ventana que llego"
        );
        assert!(
            meter.frames() > 0,
            "hay marca de primera muestra y el contador dice que no llego nada"
        );
    }

    /// La marca es la de la **primera** ventana, no la de la ultima. Si se sobrescribiera,
    /// el numero seria "hace cuanto llego audio" y no "cuanto tardo en empezar a llegar".
    #[test]
    fn la_marca_es_la_de_la_primera_ventana_y_no_se_mueve() {
        let meter = Meter::new();
        meter.push(&[0.0; 480]);
        let primera = meter.first_sample_ms();

        std::thread::sleep(std::time::Duration::from_millis(20));
        meter.push(&[0.0; 480]);

        assert_eq!(meter.first_sample_ms(), primera);
    }

    /// Un arranque mas rapido que el reloj no puede leerse como "no ha llegado nada".
    #[test]
    fn un_arranque_instantaneo_no_se_confunde_con_no_haber_arrancado() {
        assert_eq!(milis(0), None);
        assert_eq!(milis(1), Some(0), "un microsegundo son cero milisegundos, pero llego");
    }

    #[test]
    fn sin_muestras_el_contador_dice_que_no_llega_nada() {
        let meter = Meter::new();
        assert_eq!(meter.frames(), 0);
        assert_eq!(meter.read(), Level::SILENT);

        meter.push(&[0.0; 480]);
        assert_eq!(meter.frames(), 480, "llegan muestras aunque sean de silencio");
    }
}
