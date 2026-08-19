use serde::Serialize;
use sysinfo::System;

mod budget;
mod recommendation;

pub use recommendation::{recommend, HardwareFacts, Recommendation};

use crate::platform::{self, GpuInfo};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HardwareReport {
    pub os: String,
    pub cpu_brand: String,
    pub logical_cores: usize,
    pub total_ram_mb: u64,
    pub available_ram_mb: u64,
    pub gpus: Vec<GpuInfo>,
    /// Memoria de video utilizable como presupuesto para un modelo. Es `None` cuando no
    /// hay ninguna grafica con memoria propia: la de una integrada sale de la RAM del
    /// sistema y contarla seria sumar dos veces la misma memoria.
    pub dedicated_vram_mb: Option<u64>,
    pub recommendation: Recommendation,
}

const BYTES_PER_MB: u64 = 1024 * 1024;

pub fn detect() -> HardwareReport {
    let mut system = System::new_all();
    system.refresh_memory();

    let cpu_brand = system
        .cpus()
        .first()
        .map(|cpu| cpu.brand().trim().to_owned())
        .unwrap_or_else(|| "CPU desconocida".to_owned());

    let os = format!(
        "{} {}",
        System::name().unwrap_or_else(|| "SO desconocido".to_owned()),
        System::os_version().unwrap_or_default()
    )
    .trim()
    .to_owned();

    let gpus = platform::detect_gpus();
    let dedicated_vram_mb = platform::best_gpu(&gpus).and_then(GpuInfo::usable_vram_mb);

    let facts = HardwareFacts {
        logical_cores: system.cpus().len(),
        total_ram_mb: system.total_memory() / BYTES_PER_MB,
        available_ram_mb: system.available_memory() / BYTES_PER_MB,
        dedicated_vram_mb,
    };

    HardwareReport {
        os,
        cpu_brand,
        logical_cores: facts.logical_cores,
        total_ram_mb: facts.total_ram_mb,
        available_ram_mb: facts.available_ram_mb,
        gpus,
        dedicated_vram_mb,
        recommendation: recommend(&facts),
    }
}
