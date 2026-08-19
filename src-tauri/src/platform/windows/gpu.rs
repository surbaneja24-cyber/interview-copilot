//! Enumeracion de adaptadores graficos con DXGI.
//!
//! Se usa DXGI y no WMI porque es la API que reporta la memoria tal y como la ve el
//! runtime de graficos, que es lo que de verdad limita a llama.cpp o a whisper.cpp.

use ::windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1, DXGI_ADAPTER_DESC1};

use crate::platform::GpuInfo;

const BYTES_PER_MB: u64 = 1024 * 1024;

/// Valor de `DXGI_ADAPTER_FLAG_SOFTWARE`. Se compara contra el campo `Flags` del
/// descriptor para descartar el adaptador software (WARP), que no es hardware real.
const ADAPTER_FLAG_SOFTWARE: u32 = 2;

/// Umbral por encima del cual se considera que la memoria del adaptador es suya y no
/// una reserva de la RAM del sistema.
///
/// No existe ningun campo en DXGI que diga "soy integrada", y el dato crudo enganna: la
/// Radeon 610M de la maquina de referencia declara 2022 MB de `DedicatedVideoMemory`
/// que en realidad son un recorte de los 8 GB del sistema. Tomarlos por buenos llevaria
/// a recomendar un modelo en GPU sobre memoria ya contada como RAM.
///
/// Se decide por tamano y no por proporcion: la relacion entre memoria dedicada y
/// compartida tampoco distingue (una grafica dedicada de 4 GB en un equipo de 32 GB
/// tambien declara mas compartida que dedicada). El tamano es ademas lo unico que
/// importa de verdad, porque por debajo de este umbral no cabe un modelo util junto
/// con whisper, asi que la clasificacion deja de tener consecuencias.
const DISCRETE_VRAM_MB_THRESHOLD: u64 = 3072;

fn is_discrete(dedicated_vram_mb: u64) -> bool {
    dedicated_vram_mb >= DISCRETE_VRAM_MB_THRESHOLD
}

pub fn detect_gpus() -> Vec<GpuInfo> {
    match enumerate() {
        Ok(gpus) => gpus,
        Err(err) => {
            // Que falle DXGI no puede tumbar la aplicacion: sin datos de GPU la
            // recomendacion cae al presupuesto de RAM, que es el camino conservador.
            log::warn!("no se pudo enumerar adaptadores con DXGI: {err}");
            Vec::new()
        }
    }
}

fn enumerate() -> ::windows::core::Result<Vec<GpuInfo>> {
    let mut gpus = Vec::new();

    // SAFETY: llamadas COM de DXGI. El factory se libera solo al salir de scope y los
    // adaptadores son punteros validos mientras dure el bucle.
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;

        let mut index = 0u32;
        // EnumAdapters1 devuelve DXGI_ERROR_NOT_FOUND cuando se acaban: se sale del bucle.
        while let Ok(adapter) = factory.EnumAdapters1(index) {
            index += 1;

            let mut desc = DXGI_ADAPTER_DESC1::default();
            if let Err(err) = adapter.GetDesc1(&mut desc) {
                log::warn!("descriptor del adaptador {index} ilegible: {err}");
                continue;
            }

            if desc.Flags & ADAPTER_FLAG_SOFTWARE != 0 {
                continue;
            }

            let dedicated_vram_mb = desc.DedicatedVideoMemory as u64 / BYTES_PER_MB;
            let shared_memory_mb = desc.SharedSystemMemory as u64 / BYTES_PER_MB;
            let name = describe(&desc.Description);

            log::info!(
                "GPU detectada: {name} — dedicada {dedicated_vram_mb} MB, \
                 compartida {shared_memory_mb} MB, flags {:#x}",
                desc.Flags
            );

            gpus.push(GpuInfo {
                name,
                dedicated_vram_mb,
                shared_memory_mb,
                discrete: is_discrete(dedicated_vram_mb),
            });
        }
    }

    Ok(gpus)
}

/// El nombre viene como array de UTF-16 terminado en NUL y relleno de ceros.
fn describe(raw: &[u16]) -> String {
    let end = raw.iter().position(|c| *c == 0).unwrap_or(raw.len());
    String::from_utf16_lossy(&raw[..end]).trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorta_el_nombre_en_el_nul() {
        let mut raw = [0u16; 128];
        for (slot, ch) in raw.iter_mut().zip("AMD Radeon 610M".encode_utf16()) {
            *slot = ch;
        }
        assert_eq!(describe(&raw), "AMD Radeon 610M");
    }

    /// Medicion real de la maquina de referencia (18-08-2026): la Radeon 610M declara
    /// 2022 MB de memoria "dedicada" que son un recorte de la RAM del sistema. Si este
    /// test se cae, alguien ha bajado el umbral y la app volvera a prometer una GPU que
    /// no existe.
    #[test]
    fn la_radeon_610m_no_cuenta_como_dedicada() {
        assert!(!is_discrete(2022));
    }

    #[test]
    fn una_gpu_dedicada_normal_si_cuenta() {
        assert!(is_discrete(6144)); // RTX 3060 de 6 GB
        assert!(is_discrete(12_288)); // RTX 3060 de 12 GB
    }

    #[test]
    fn nombre_sin_nul_no_desborda() {
        let raw: Vec<u16> = "GPU".encode_utf16().collect();
        assert_eq!(describe(&raw), "GPU");
    }

    /// No comprueba que haya GPU: en un CI sin adaptador la lista vacia es correcta.
    /// Lo que comprueba es que enumerar no entre en panic ni cuelgue. Imprime lo que
    /// encuentra para poder calibrar `DISCRETE_VRAM_MB_THRESHOLD` con datos reales:
    /// `cargo test -- --nocapture enumerar`.
    #[test]
    fn enumerar_no_revienta() {
        let gpus = detect_gpus();
        println!("adaptadores encontrados: {}", gpus.len());
        for gpu in &gpus {
            println!(
                "  {} | dedicada {} MB | compartida {} MB | clasificada como {}",
                gpu.name,
                gpu.dedicated_vram_mb,
                gpu.shared_memory_mb,
                if gpu.discrete {
                    "dedicada"
                } else {
                    "integrada"
                }
            );
            assert!(!gpu.name.is_empty());
        }
    }
}
