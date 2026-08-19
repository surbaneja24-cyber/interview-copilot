//! Capa de plataforma. Es el unico sitio del proyecto donde se permite codigo especifico
//! de un sistema operativo (§27 del spec). Todo lo de arriba consume estos tipos, que son
//! iguales en las tres plataformas, y nunca llama a una API nativa directamente.

use serde::Serialize;

#[cfg(target_os = "windows")]
#[path = "windows/mod.rs"]
mod imp;

#[cfg(not(target_os = "windows"))]
#[path = "unsupported.rs"]
mod imp;

pub use imp::detect_gpus;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuInfo {
    pub name: String,
    /// Memoria propia del adaptador. En una grafica integrada esto es una reserva de la
    /// RAM del sistema, no memoria adicional.
    pub dedicated_vram_mb: u64,
    /// RAM del sistema que el adaptador puede usar prestada.
    pub shared_memory_mb: u64,
    /// Heuristica: ver `is_discrete` en la implementacion de cada plataforma.
    pub discrete: bool,
}

impl GpuInfo {
    /// Memoria que se puede usar como presupuesto para un modelo, o `None` si esta
    /// grafica no aporta memoria propia utilizable.
    pub fn usable_vram_mb(&self) -> Option<u64> {
        if self.discrete {
            Some(self.dedicated_vram_mb)
        } else {
            None
        }
    }
}

/// La grafica con mas memoria propia utilizable, si hay alguna.
pub fn best_gpu(gpus: &[GpuInfo]) -> Option<&GpuInfo> {
    gpus.iter()
        .max_by_key(|gpu| gpu.usable_vram_mb().unwrap_or(0))
}
