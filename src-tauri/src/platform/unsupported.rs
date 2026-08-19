//! Stub para macOS y Linux. Devuelve una lista vacia en vez de inventar una API
//! multiplataforma falsa (§32 del spec): sin datos, la recomendacion cae a RAM de
//! sistema, que es el comportamiento correcto y honesto.

use super::GpuInfo;

pub fn detect_gpus() -> Vec<GpuInfo> {
    log::info!("deteccion de GPU no implementada en esta plataforma");
    Vec::new()
}
