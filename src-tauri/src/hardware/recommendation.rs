use serde::Serialize;

/// Datos crudos de los que depende la recomendacion. Se separan de `HardwareReport`
/// para poder testear la logica sin tocar el sistema real.
#[derive(Debug, Clone, Copy)]
pub struct HardwareFacts {
    pub logical_cores: usize,
    pub total_ram_mb: u64,
    pub available_ram_mb: u64,
    /// Solo VRAM **dedicada**. La memoria de una grafica integrada sale de la RAM del
    /// sistema, asi que se pasa como `None`: contarla seria sumar dos veces la misma
    /// memoria y acabar recomendando un modelo que no cabe.
    pub dedicated_vram_mb: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum ExecutionProfile {
    Local,
    Hybrid,
    Cloud,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Recommendation {
    pub profile: ExecutionProfile,
    pub stt_model: &'static str,
    /// `None` si no cabe ningun modelo local con este hardware.
    pub local_llm: Option<&'static str>,
    /// Si es falso, el LLM local sirve para practicar pero no para una entrevista en vivo.
    pub realtime_local_llm: bool,
    pub reasons: Vec<String>,
}

/// RAM que hay que dejar libre para Windows y para la propia aplicacion antes de
/// repartir lo que queda al modelo. Medido en la maquina de referencia: el sistema en
/// reposo con navegador y editor abiertos ronda los 4 GB.
const RESERVED_RAM_MB: u64 = 4096;

/// Un modelo cuantizado necesita su tamano en disco mas el KV cache del contexto.
/// Cifras para Q4_K_M con ventana de 8k.
const MB_FOR_8B: u64 = 6144;
const MB_FOR_3B: u64 = 3072;
const MB_FOR_1B: u64 = 1536;

/// Por debajo de esta VRAM dedicada, la generacion cae a CPU y deja de haber ninguna
/// posibilidad de responder en los 2-4 s que pide el producto.
const VRAM_MB_FOR_REALTIME: u64 = 6144;

pub fn recommend(facts: &HardwareFacts) -> Recommendation {
    let mut reasons = Vec::new();

    // Una GPU dedicada aporta memoria propia; una integrada reparte la misma RAM del
    // sistema. Quien rellena los datos es el responsable de esa distincion.
    let dedicated_vram = facts.dedicated_vram_mb.filter(|mb| *mb >= MB_FOR_1B);
    let budget_mb = match dedicated_vram {
        Some(vram) => {
            reasons.push(format!("GPU con {vram} MB de VRAM dedicada."));
            vram
        }
        None => {
            let budget = facts.total_ram_mb.saturating_sub(RESERVED_RAM_MB);
            reasons.push(format!(
                "Sin VRAM dedicada detectada: el modelo tendria que correr en CPU. \
                 De {} MB de RAM total quedan {budget} MB tras reservar {RESERVED_RAM_MB} MB \
                 para el sistema y la aplicacion.",
                facts.total_ram_mb
            ));
            budget
        }
    };

    let local_llm = if budget_mb >= MB_FOR_8B {
        Some("llama-3.1-8b-instruct-q4_k_m")
    } else if budget_mb >= MB_FOR_3B {
        Some("qwen2.5-3b-instruct-q4_k_m")
    } else if budget_mb >= MB_FOR_1B {
        Some("llama-3.2-1b-instruct-q4_k_m")
    } else {
        None
    };

    let realtime_local_llm =
        local_llm.is_some() && dedicated_vram.is_some_and(|vram| vram >= VRAM_MB_FOR_REALTIME);

    match local_llm {
        None => reasons.push(
            "No cabe ningun modelo local con este presupuesto de memoria: el LLM tiene que \
             ir en la nube."
                .to_owned(),
        ),
        Some(model) if !realtime_local_llm => reasons.push(format!(
            "Cabe {model}, pero generando en CPU la respuesta tardara del orden de decenas \
             de segundos. Sirve para el modo practica, no para una entrevista en vivo."
        )),
        Some(model) => reasons.push(format!(
            "{model} cabe en la GPU y puede responder en tiempo real."
        )),
    }

    // Whisper compite por los mismos nucleos que todo lo demas, asi que el modelo de STT
    // se elige por CPU disponible y no por memoria.
    let stt_model = if facts.logical_cores >= 8 && facts.total_ram_mb >= 16_384 {
        "whisper-small"
    } else if facts.logical_cores >= 4 {
        "whisper-base"
    } else {
        "whisper-tiny"
    };
    reasons.push(format!(
        "{stt_model} para transcribir, con {} hilos logicos disponibles.",
        facts.logical_cores
    ));

    let profile = if realtime_local_llm {
        ExecutionProfile::Local
    } else if local_llm.is_some() {
        ExecutionProfile::Hybrid
    } else {
        ExecutionProfile::Cloud
    };

    if facts.available_ram_mb < 1024 {
        reasons.push(format!(
            "Aviso: ahora mismo solo hay {} MB de RAM libre. Cierra aplicaciones antes de \
             empezar una entrevista.",
            facts.available_ram_mb
        ));
    }

    Recommendation {
        profile,
        stt_model,
        local_llm,
        realtime_local_llm,
        reasons,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(cores: usize, ram_mb: u64, dedicated_vram_mb: Option<u64>) -> HardwareFacts {
        HardwareFacts {
            logical_cores: cores,
            total_ram_mb: ram_mb,
            available_ram_mb: ram_mb / 2,
            dedicated_vram_mb,
        }
    }

    /// El caso concreto de la maquina de desarrollo: 8 GB fisicos de los que el sistema
    /// solo expone 5,74, iGPU sin VRAM dedicada.
    #[test]
    fn portatil_de_referencia_recomienda_hybrid() {
        let rec = recommend(&facts(8, 5878, None));
        assert_eq!(rec.profile, ExecutionProfile::Hybrid);
        assert_eq!(rec.local_llm, Some("llama-3.2-1b-instruct-q4_k_m"));
        assert!(!rec.realtime_local_llm);
        assert_eq!(rec.stt_model, "whisper-base");
    }

    #[test]
    fn gpu_dedicada_grande_recomienda_local() {
        let rec = recommend(&facts(16, 32_768, Some(12_288)));
        assert_eq!(rec.profile, ExecutionProfile::Local);
        assert_eq!(rec.local_llm, Some("llama-3.1-8b-instruct-q4_k_m"));
        assert!(rec.realtime_local_llm);
        assert_eq!(rec.stt_model, "whisper-small");
    }

    #[test]
    fn maquina_minima_cae_a_cloud() {
        let rec = recommend(&facts(2, 4096, None));
        assert_eq!(rec.profile, ExecutionProfile::Cloud);
        assert_eq!(rec.local_llm, None);
        assert_eq!(rec.stt_model, "whisper-tiny");
    }

    /// Una VRAM dedicada mas pequena que el modelo mas pequeno no cambia nada: el
    /// presupuesto vuelve a salir de la RAM del sistema.
    #[test]
    fn vram_dedicada_insuficiente_se_ignora() {
        let con_gpu_minuscula = recommend(&facts(8, 5878, Some(1024)));
        let sin_gpu = recommend(&facts(8, 5878, None));
        assert_eq!(con_gpu_minuscula.local_llm, sin_gpu.local_llm);
        assert_eq!(con_gpu_minuscula.profile, sin_gpu.profile);
    }

    #[test]
    fn gpu_dedicada_pequena_no_promete_tiempo_real() {
        let rec = recommend(&facts(8, 16_384, Some(4096)));
        assert!(!rec.realtime_local_llm);
        assert_eq!(rec.profile, ExecutionProfile::Hybrid);
    }

    #[test]
    fn avisa_cuando_apenas_queda_ram_libre() {
        let mut low = facts(8, 5878, None);
        low.available_ram_mb = 400;
        let rec = recommend(&low);
        assert!(rec
            .reasons
            .iter()
            .any(|r| r.contains("400 MB de RAM libre")));
    }
}
