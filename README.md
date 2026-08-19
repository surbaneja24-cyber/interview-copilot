# Interview Copilot

Copiloto de entrevistas de escritorio, local-first. Indexa tu CV, la oferta y tus notas;
escucha la entrevista, detecta las preguntas y sugiere qué responder a partir de tu
experiencia real.

Estado: **en construcción, Fase 0 del roadmap.** Nada funciona todavía de extremo a extremo.

- `docs/ARCHITECTURE.md` — decisiones de stack y por qué
- `docs/ROADMAP.md` — fases y criterio de cierre de cada una
- `docs/SETUP.md` — qué instalar antes de poder compilar

## Arranque rápido

```
npm install
```

```
npm run tauri dev
```

Requiere la toolchain de `docs/SETUP.md` (Rust, MSVC Build Tools, CMake). Sin ella solo
arranca el frontend con `npm run dev`.

## Principios

La aplicación no inventa experiencia profesional. Si el retriever no encuentra nada
relevante en tu contexto, la app lo dice y ofrece una estructura de respuesta en lugar de
fabricar un ejemplo.

Tus datos —CV, audio, transcripciones, respuestas— se guardan solo en tu equipo. Si activas
un proveedor en la nube, la interfaz declara qué sale del equipo antes de que salga.

La protección de captura de pantalla usa `SetWindowDisplayAffinity` de Windows y funciona
sobre los mecanismos de captura que respetan esa política del sistema. No es invisibilidad:
no protege frente a una cámara apuntando al monitor ni frente a métodos que ignoren esa
política.
