//! De lo que entrega la tarjeta a lo que comen los modelos: 16 kHz, mono, `f32`.
//!
//! Whisper y Silero trabajan los dos a 16 kHz mono, y una tarjeta de sonido entrega lo que
//! le da la gana —48 kHz y dos canales en la maquina de referencia—. La conversion vive
//! aparte del resto del audio porque es la unica parte de la cadena que se puede probar
//! con numeros escritos a mano.
//!
//! No es un remuestreador de calidad de audio y no pretende serlo: no hay filtro de
//! ventana ni interpolacion polifasica. Para voz que va a un detector y a un
//! transcriptor, promediar bloques es suficiente y cuesta una suma por muestra, que es lo
//! que importa cuando esto corre mientras el usuario esta hablando.

/// Frecuencia a la que trabajan Silero y whisper.cpp.
pub const TARGET_HZ: u32 = 16_000;

/// Convierte un bloque intercalado a mono de 16 kHz, anadiendolo a `out`.
///
/// `out` se recibe y no se devuelve para poder reutilizar el mismo vector entre bloques:
/// esto corre por cada trozo de audio que llega, y reservar memoria cada vez es la clase
/// de gasto que se nota justo cuando el equipo va justo.
pub fn to_mono_16k(interleaved: &[f32], channels: u16, sample_rate: u32, out: &mut Vec<f32>) {
    let channels = usize::from(channels.max(1));
    let frames = interleaved.len() / channels;
    if frames == 0 {
        return;
    }

    // Mezcla de canales: la media, no el primer canal. Un microfono estereo con la voz
    // centrada la trae en los dos, pero uno mal cableado la trae solo en uno, y quedarse
    // con el izquierdo daria silencio sin que nadie entienda por que.
    let mono = (0..frames).map(|frame| {
        let start = frame * channels;
        let sum: f32 = interleaved[start..start + channels].iter().sum();
        sum / channels as f32
    });

    if sample_rate == TARGET_HZ {
        out.extend(mono);
        return;
    }

    // Promedio de bloques de tamano variable: con 48 kHz salen bloques de 3 exactos, y con
    // frecuencias que no son multiplo -44,1 kHz- el tamano alterna entre 2 y 3 en vez de
    // acumular deriva.
    let ratio = f64::from(sample_rate) / f64::from(TARGET_HZ);
    let salida = (frames as f64 / ratio).floor() as usize;

    let mono: Vec<f32> = mono.collect();
    for index in 0..salida {
        let start = (index as f64 * ratio).round() as usize;
        let end = (((index + 1) as f64 * ratio).round() as usize).min(mono.len());
        if start >= end {
            continue;
        }
        let bloque = &mono[start..end];
        out.push(bloque.iter().sum::<f32>() / bloque.len() as f32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn de_48k_estereo_a_16k_mono_divide_por_tres() {
        // Medio segundo a 48 kHz, dos canales.
        let entrada = vec![0.0f32; 48_000];
        let mut out = Vec::new();
        to_mono_16k(&entrada, 2, 48_000, &mut out);

        // 48 000 muestras / 2 canales = 24 000 marcos; a 16 kHz son 8 000.
        assert_eq!(out.len(), 8_000);
    }

    #[test]
    fn a_16k_mono_no_toca_nada() {
        let entrada = vec![0.25, -0.5, 0.75];
        let mut out = Vec::new();
        to_mono_16k(&entrada, 1, TARGET_HZ, &mut out);
        assert_eq!(out, entrada);
    }

    /// La voz puede venir en un solo canal. Quedarse con el izquierdo daria silencio.
    #[test]
    fn mezcla_los_canales_en_vez_de_quedarse_con_uno() {
        // Izquierdo mudo, derecho con senal.
        let entrada = vec![0.0, 1.0, 0.0, 1.0];
        let mut out = Vec::new();
        to_mono_16k(&entrada, 2, TARGET_HZ, &mut out);
        assert_eq!(out, vec![0.5, 0.5]);
    }

    /// Con 44,1 kHz la razon no es entera. Lo que no puede pasar es que el error se
    /// acumule: un desfase creciente descoloca la marca de tiempo de la transcripcion.
    #[test]
    fn una_frecuencia_que_no_es_multiplo_no_acumula_deriva() {
        let segundos = 10;
        let entrada = vec![0.0f32; 44_100 * segundos];
        let mut out = Vec::new();
        to_mono_16k(&entrada, 1, 44_100, &mut out);

        let esperado = TARGET_HZ as usize * segundos;
        let error = out.len().abs_diff(esperado);
        assert!(
            error <= 1,
            "10 s dieron {} muestras en vez de {esperado}: se desvia {error}",
            out.len()
        );
    }

    /// Un tono continuo tiene que seguir siendo un tono: si el promediado estuviera mal
    /// alineado, la amplitud se desplomaria.
    #[test]
    fn un_tono_conserva_su_amplitud() {
        let entrada: Vec<f32> = (0..48_000)
            .map(|index| (std::f32::consts::TAU * 220.0 * index as f32 / 48_000.0).sin())
            .collect();

        let mut out = Vec::new();
        to_mono_16k(&entrada, 1, 48_000, &mut out);

        let pico = out.iter().fold(0.0f32, |max, sample| max.max(sample.abs()));
        assert!(
            pico > 0.9,
            "el pico bajo a {pico}: el promediado esta destruyendo la senal"
        );
    }

    #[test]
    fn un_bloque_vacio_no_revienta() {
        let mut out = Vec::new();
        to_mono_16k(&[], 2, 48_000, &mut out);
        assert!(out.is_empty());
    }
}
