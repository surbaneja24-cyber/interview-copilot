//! Cuanta memoria ocupa la aplicacion con todos los modelos dentro.
//!
//! Es la medicion que decide si el MVP 1 es usable en esta maquina o hay que cargar y
//! soltar modelos por etapas. Hasta ahora cada pieza se habia medido por separado —el
//! modelo de embeddings ~1 GB, whisper ~200 MB, la app 261 MB en desarrollo— y sumarlas a
//! mano no vale: los modelos reservan memoria al inferir, no al cargarse, y `ARCHITECTURE`
//! §0 dice que el cuello de botella real de este equipo no es la velocidad sino la RAM.
//!
//! No es un test que pueda fallar por si solo: es un banco de medida, como
//! `embedding/benchmark.rs`. Lo que hace es dejar los numeros por escrito.

#[cfg(test)]
mod tests {
    use sysinfo::{Pid, ProcessRefreshKind, RefreshKind, System};

    /// Memoria residente de este proceso, en MB.
    fn resident_mb(system: &mut System) -> u64 {
        let pid = Pid::from_u32(std::process::id());
        system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
        system
            .process(pid)
            .map(|process| process.memory() / 1024 / 1024)
            .unwrap_or(0)
    }

    fn disponible_mb(system: &mut System) -> u64 {
        system.refresh_memory();
        system.available_memory() / 1024 / 1024
    }

    /// Carga los tres modelos en el mismo proceso y apunta cuanto ocupa cada paso.
    ///
    /// Necesita los modelos ya descargados. Por defecto los busca donde los deja la
    /// aplicacion; con `INTERVIEW_COPILOT_MODELS` se le puede dar otra carpeta.
    ///
    /// `cargo test --lib -- --ignored --nocapture presupuesto_de_memoria`
    #[test]
    #[ignore = "carga todos los modelos a la vez"]
    fn presupuesto_de_memoria() {
        let models = std::env::var("INTERVIEW_COPILOT_MODELS").unwrap_or_else(|_| {
            format!(
                "{}\\dev.urbaneja.interviewcopilot\\models",
                std::env::var("APPDATA").expect("APPDATA")
            )
        });
        let models = std::path::PathBuf::from(models);
        assert!(models.is_dir(), "no existe {}", models.display());

        let mut system = System::new_with_specifics(
            RefreshKind::nothing().with_processes(ProcessRefreshKind::everything()),
        );

        let arranque = resident_mb(&mut system);
        println!("proceso de test, sin modelos: {arranque} MB");
        println!("RAM disponible en el equipo: {} MB", disponible_mb(&mut system));

        // 1. Embeddings. Es el mas grande y el unico que ya se sabia soltar.
        let embedder = crate::embedding::LocalEmbeddingProvider::new(&models)
            .expect("cargar el modelo de embeddings");
        let con_embeddings = resident_mb(&mut system);
        println!(
            "+ embeddings: {con_embeddings} MB (+{})",
            con_embeddings.abs_diff(arranque)
        );

        // Una inferencia de verdad: los modelos reservan de mas al usarse, no al cargarse.
        {
            use crate::embedding::EmbeddingProvider;
            embedder
                .embed_query("¿Cuéntame un proyecto complicado?")
                .expect("inferir");
        }
        let tras_inferir = resident_mb(&mut system);
        println!(
            "+ una consulta: {tras_inferir} MB ({}{})",
            if tras_inferir >= con_embeddings { "+" } else { "-" },
            tras_inferir.abs_diff(con_embeddings)
        );

        // 2. El VAD.
        let vad = models.join(crate::audio::vad::MODEL_FILE);
        let mut silero = crate::audio::vad::Silero::load(&vad).expect("cargar el VAD");
        silero
            .probability(&[0.0; crate::audio::vad::FRAME_SAMPLES])
            .expect("inferir");
        let con_vad = resident_mb(&mut system);
        println!(
            "+ VAD: {con_vad} MB ({}{})",
            if con_vad >= tras_inferir { "+" } else { "-" },
            con_vad.abs_diff(tras_inferir)
        );

        // 3. whisper, con una transcripcion de verdad.
        let whisper_model = crate::stt::MODELS
            .iter()
            .find(|model| model.is_downloaded(&models))
            .expect("ningun modelo de whisper descargado");
        let mut whisper =
            crate::stt::LocalWhisper::load(&whisper_model.path(&models), whisper_model.id)
                .expect("cargar whisper");
        let con_whisper = resident_mb(&mut system);
        println!(
            "+ whisper ({}): {con_whisper} MB ({}{})",
            whisper_model.id,
            if con_whisper >= con_vad { "+" } else { "-" },
            con_whisper.abs_diff(con_vad)
        );

        {
            use crate::stt::SttProvider;
            // Tres segundos de silencio bastan para que reserve sus buffers de trabajo.
            whisper
                .transcribe(&vec![0.0; 16_000 * 3], Some("es"))
                .expect("transcribir");
        }
        let todo = resident_mb(&mut system);
        // Puede bajar: whisper suelta sus buffers de carga al terminar. Restar a lo bruto
        // desborda, que es como se descubrio.
        println!(
            "+ una transcripción: {todo} MB ({}{})",
            if todo >= con_whisper { "+" } else { "-" },
            todo.abs_diff(con_whisper)
        );

        println!("---");
        println!("total con los tres modelos cargados y usados: {todo} MB");
        println!("RAM disponible ahora en el equipo: {} MB", disponible_mb(&mut system));
        println!(
            "y esto sin el LLM, que en modo LOCAL sumaria ~2 GB más (ARCHITECTURE §0)"
        );
    }
}
