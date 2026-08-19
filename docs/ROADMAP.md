# Interview Copilot — Roadmap técnico

Orden de §34 del spec: motor de entrevista → STT local → LLM → RAG → UI en vivo →
protección de ventana → práctica → analítica. El criterio para cerrar cada hito es que
algo **funcione de extremo a extremo**, no que el código exista.

## Fase 0 — Toolchain y esqueleto

Requiere instalación manual (ver `docs/SETUP.md`): Rust, MSVC Build Tools con el workload
de C++, CMake.

- [x] Toolchain instalada y verificada (Rust 1.97.1, MSVC 14.44, CMake 4.4.2)
- [x] Proyecto Tauri 2 + React + TS estricto arrancando en `npm run tauri dev`
- [x] ESLint con reglas type-checked estrictas, sin avisos
- [x] SQLite con migraciones y ruta de datos en `%APPDATA%`
- [x] `cargo clippy` sin avisos, con `npm run lint:rust`. No hay CI todavia, asi que es
      un comando y no una barrera automatica; lo que si hace es fallar con `-D warnings`
      en vez de dejar pasar los avisos

**Hito conseguido:** la ventana abre y escribe en la base de datos. Verificado: proceso vivo,
`user_version = 1`, tabla `projects` creada, WAL activo. Huella de memoria 261 MB en dev.

Lo que encontro clippy al pasarlo por primera vez, el 2026-08-19, y que conviene no
repetir:

- **El crate declaraba `rust-version = "1.77.2"` y no compilaba con esa version.** Usa
  `Option::is_none_or`, estable desde 1.82. La cifra se puso a ojo y nadie la comprobo;
  cualquiera que hubiera intentado compilar con la version prometida se habria estrellado.
- **Un `_ => false` en `ProviderKind::sends_data_outside`.** Anadir un proveedor de nube
  nuevo lo habria declarado en silencio como que no saca datos del equipo. Ahora las
  variantes se enumeran todas, para que anadir una rompa la compilacion y obligue a
  decidir. Es el tipo de comodin que parece limpieza y es una trampa.

## Fase 1 — Detección de hardware y Settings

Va antes que la IA a propósito: es lo que determina qué modelos tiene sentido ofrecer, y
en esta máquina la respuesta cambia el resto del producto.

- [x] Detección de CPU, núcleos, RAM total y disponible, SO
- [x] Detección de GPU y VRAM por DXGI, distinguiendo dedicada de integrada
- [x] Motor de recomendación: perfil de ejecución + modelo STT + modelo LLM sugeridos
- [x] Aviso honesto cuando el hardware no da para LOCAL en tiempo real
- [ ] Pantalla de Settings completa (§19), persistida — la tabla `settings` y los ajustes
      del LLM ya están (Fase 3); faltan los de interfaz, audio y rendimiento

**Hito conseguido:** la app dice, con números, qué puede correr esta máquina.

Hallazgo que justificó el trabajo: DXGI declara 2022 MB de `DedicatedVideoMemory` para la
Radeon 610M integrada, memoria que en realidad es un recorte de los 8 GB del sistema.
Tomar ese dato al pie de la letra habría hecho recomendar un modelo en GPU sobre memoria
ya contada como RAM. Está fijado en un test de regresión.

## Fase 2 — Proyectos y base de conocimiento

- [x] CRUD de proyectos (§20)
- [x] Chunking con solape y metadatos de procedencia
- [x] Índice vectorial en sqlite-vec, con búsqueda por vecino más cercano
- [x] Proveedor de embeddings local con fastembed, detrás del trait de §18
- [x] Banco de pruebas de modelos y elección con datos (ver `ARCHITECTURE.md` §2.1)
- [x] Carga de documentos: PDF, DOCX, TXT, MD
- [x] Pipeline de indexado: documento → chunks → embeddings → índice
- [x] Buscador manual sobre el índice, para poder verificar la calidad del retrieval
- [x] Carga perezosa del modelo de embeddings y liberación manual (~1 GB)
- [x] Datos de contacto fuera del índice (§31), medido contra el CV real
- [ ] Formularios de Candidate Profile y Job Profile (§5) — los documentos ya cubren el
      caso principal; los campos estructurados quedan para cuando el generador los use

**Hito alcanzado a medias.** La búsqueda funciona sobre un CV real y devuelve fragmentos
razonables (8 fragmentos, mediana de 236 caracteres, sin texto roto), pero la calidad no es
la que debería. Queda **pendiente y consciente**:

- **Encabezados en Mayúscula Inicial.** El detector solo reconoce secciones en MAYÚSCULAS.
  Un CV que titule "Experiencia Laboral" en vez de "EXPERIENCIA LABORAL" no se beneficia de
  la frontera de sección. Ampliarlo requiere una heurística más frágil (línea corta, sin
  punto final, seguida de contenido más largo) y no se ha medido si acierta más de lo que
  falla.
- **Calibrar `TARGET_CHARS` (700) y `MERGE_ACROSS_PARAGRAPHS_BELOW` (300).** Ambos se
  eligieron razonando sobre currículums genéricos, antes de ver uno real. Nunca se han
  medido contra un corpus de verdad.
- **Extracción de PDF.** `pdf-extract` falla con PDFs que posicionan glifo a glifo: un CV
  real dio 68 letras sueltas sobre 417 palabras. Hay detección que rechaza el fichero y
  pide .docx, pero la solución de fondo es cambiar a PDFium, lo que implica distribuir una
  biblioteca nativa con la aplicación.
**Cerrado el 2026-08-19: los datos de contacto ya no entran en el índice.** Correos,
teléfonos y URLs de perfil se quitan antes de trocear (`rag/contact.rs`), y la UI dice
cuántos dejó fuera. La regla hubo que corregirla dos veces contra el CV real —primero
tiraba líneas enteras y dejaba el correo dentro; luego se topó con que `pdf-extract` parte
el correo por el espacio— y el detalle está en `ARCHITECTURE.md` §3.2. Sigue sin
detectarse un nombre suelto, y eso es deliberado: ninguna regla separa "SANTIAGO URBANEJA"
de "EXPERIENCIA LABORAL" sin llevarse por delante los encabezados de sección.

Tres cosas que costaron más de lo previsto y conviene no repetir:

- El aviso de §6 se diseñó como umbral sobre la similitud de recuperación. Se midió con
  preguntas sin respuesta en el corpus y **ninguna señal separa** (ni el despegue, ni la
  similitud absoluta, ni un reranker cross-encoder). Movido a la capa de generación como
  cita verificada. El reranker queda descartado de paso: ordena peor que el bi-encoder.


- El modelo elegido a ojo (`e5-small`) acertaba 2 de 6 preguntas. Sustituido por `e5-base`,
  que acierta 6 de 6. La medición está automatizada y actúa como guardia de regresión.
- El troceado tenía un fallo de límite: al añadir el solape por delante, un trozo cuya
  unidad ya medía el máximo se pasaba del tope. Corregido reservando el hueco del solape
  en el límite de la unidad.

## Fase 3 — Providers de LLM

- [x] Trait `LlmProvider` con `generate`, `stream_chat` y `models`. `generate` está
      construido sobre `stream_chat` a propósito: dos implementaciones paralelas acaban
      divergiendo, y hay un test que comprueba que devuelven lo mismo
- [x] Proveedor local contra llama-server / Ollama
- [x] Proveedor OpenAI. **Anthropic no se implementa** — ver `ARCHITECTURE.md` §2: otro
      protocolo, ninguna forma de probarlo, y código de red sin probar no es una función
- [x] Streaming token a token hasta la UI, por `Channel` de Tauri
- [x] Claves API en el almacén de credenciales de Windows, nunca en la base de datos y sin
      ningún comando que las devuelva al frontend (§31). El borrado total de §15 las limpia
- [x] Respuesta estructurada: sugerencia, key points, follow-ups (§8), con parseo tolerante
      a vallas de código, prosa alrededor y mezcla de camelCase con snake_case
- [x] **Citas literales y su verificación** — el generador devuelve qué fragmento respalda
      cada afirmación **y un trozo copiado palabra por palabra de él**. Sin cita literal
      verificada no hay respuesta, solo el aviso de §6. Sustituye al umbral de retrieval,
      que se midió y no funciona (ver `ARCHITECTURE.md` §5)
- [x] La respuesta no aparece en pantalla hasta que sus citas están verificadas, sin
      renunciar al streaming: el prompt fija el orden de los campos del JSON y las citas
      llegan antes que el texto
- [x] Ajustes del LLM persistidos (proveedor, URL, modelo, temperatura)
- [ ] Clasificación automática del tipo de pregunta — es §7 y va en la Fase 5. Hasta
      entonces el estilo (comportamental / técnica / general) se elige a mano en la UI

**Hito conseguido a medias.** La ruta completa funciona de extremo a extremo —pregunta,
recuperación, generación con streaming, verificación de citas, respuesta en pantalla con
sus fuentes— pero **verificada contra el proveedor simulado, no contra un modelo real**.
Falta la mitad que no depende del código:

- Con **Ollama**: instalarlo y descargar un modelo (`ollama pull qwen2.5:3b-instruct`).
  El detector de hardware ya avisa de que en esta máquina tardará decenas de segundos.
- Con **OpenAI**: pegar una clave en Ajustes.

### Medido el 2026-08-19 con Ollama

Primera prueba contra un servidor real, no contra el simulador.

- **El camino local funciona**: la app lista los modelos de Ollama, persiste el cambio de
  proveedor y genera por el endpoint compatible con OpenAI.
- **`qwen2.5:3b` no arranca en esta máquina.** Pide un buffer de 1266 MB y no lo hay. El
  detector de hardware ya recomendaba un 1B; tenía razón.
- **`llama3.2:1b` se inventó la experiencia entera** y adornó con dos citas literales
  reales del CV. Se bloqueó, pero por escribir el título de la sección donde se pide el
  número, no por detectar la invención. El detalle está en `ARCHITECTURE.md` §5 porque
  cambia lo que se puede afirmar de esta capa.
- Consecuencia inmediata: citar mal y no citar ya no dan el mismo mensaje. Uno se arregla
  cambiando de modelo y el otro no.
- Latencia: entre 90 y 100 s por respuesta con el 1B en CPU. Sin sorpresa respecto a lo
  previsto en `ARCHITECTURE.md` §0.

Lo que sigue sin medirse, y hace falta un modelo capaz para ello:

- **Cuánto parafrasea.** El filtro exige cita literal. Si un modelo de 3B parafrasea casi
  siempre, rechazará respuestas buenas. Por eso las citas descartadas se cuentan y se
  enseñan en la UI en vez de desaparecer: son el dato con el que decidir si el filtro está
  bien calibrado, y esa decisión se toma con números, no a ojo.
- **Si respeta el orden de los campos.** Si no lo hace, el texto se retiene y se suelta al
  verificar, así que sigue siendo correcto — pero se pierde la sensación de streaming.
- **La latencia real por etapa**, que es lo que decide si el perfil HYBRID cumple los
  2-4 s de §10.

## Fase 4 — Audio y STT

En este orden y no otro: micrófono con medidor —lo único verificable hablando y sin
compilar nada de C++— y whisper.cpp el último, que es el que exige compilar con 4 núcleos.

- [x] Enumerar dispositivos de entrada, con el identificador estable de cpal 0.17
- [x] Captura de micrófono (`cpal`, WASAPI) en su propio hilo
- [x] Medidor de nivel: RMS y pico retenido, con su barra en Ajustes (§11)
- [ ] Loopback del sistema (WASAPI) e indicador MIC / SYSTEM AUDIO / BOTH
- [ ] VAD con detección de fin de turno
- [ ] `LocalWhisperProvider` con whisper.cpp, descarga de modelos gestionada por la app
- [ ] Transcripción incremental sobre ventanas solapadas
- [ ] Medición de latencia por etapa, visible en un panel de diagnóstico

**Hito:** hablo y el texto aparece en pantalla en tiempo real.

### Lo que ya funciona, medido el 2026-08-19

**Verificado hablando**, que es el criterio de esta fase: la barra se mueve con la voz en
Ajustes → Micrófono. Windows confirma además que el dispositivo se suelta al parar — deja
apuntado inicio y fin de cada uso, y el fin está puesto.

Medio segundo de captura del micrófono real: `Auriculares con micrófono`, 48 000 Hz, 2
canales, 47 040 muestras, RMS −47,7 dB y pico −20,9 dB con la sala en silencio. El test
que lo hace está marcado `#[ignore]` porque toma el micrófono del equipo.

Tres decisiones de esta parte, para no rehacerlas:

- **El identificador de dispositivo no es el nombre.** cpal 0.17 da un `DeviceId` estable
  entre reinicios y reconexiones, y ya marca `name()` como obsoleto. El nombre no distingue
  dos tarjetas iguales, y el identificador es lo que permitirá recordar en los ajustes qué
  micrófono eligió el usuario.
- **El nivel no viaja por un `Channel` de Tauri.** La llamada de retorno de audio entra
  cada 10 ms: serían cien mensajes por segundo para una barra que se repinta sesenta veces.
  La UI pregunta cuando va a dibujar, cada 100 ms.
- **Se cuentan las muestras recibidas, no solo el nivel.** Un micrófono silenciado por
  hardware y una sala en silencio dan la misma barra plana, y no son el mismo problema.

**Pendiente cuando entre el loopback:** el nivel de dos fuentes a la vez y el indicador de
§11 completo. Hoy la tarjeta dice sin rodeos que el audio del sistema todavía no se captura,
en vez de enseñar un selector MIC / SYSTEM / BOTH con dos opciones que no hacen nada.

## Fase 5 — Entrevista en vivo (cierre del MVP 1)

- [ ] Clasificador de preguntas: reglas primero, LLM solo ante ambigüedad (§7)
- [ ] Máquina de estados de la entrevista
- [ ] Retrieval especulativo durante las pausas
- [ ] Prefijo de prompt cacheado
- [ ] UI de entrevista: transcript, tipo de pregunta, respuesta, key points, follow-ups (§9)
- [ ] Modos de ventana Normal / Compact / Minimal (§30)
- [ ] Tema, opacidad, tamaño de fuente

**Hito: MVP 1 cerrado.** Alguien me habla por videollamada y la app sugiere qué responder.

## Fase 6 — Protección de captura

- [ ] Trait `CaptureProtection` multiplataforma con stubs para macOS y Linux
- [ ] Implementación Windows con `SetWindowDisplayAffinity`
- [ ] Modos OFF / EXCLUDE_FROM_CAPTURE / MONITOR_ONLY
- [ ] Estados de UI Active / Limited protection, con el texto exacto de §26
- [ ] Módulo de diagnóstico "Test Screen Capture Protection" (§29)
- [ ] Activación automática al entrar en entrevista, configurable
- [ ] Logs de enabled / disabled / API error / unsupported

**Hito:** compartir pantalla en Meet y comprobar en una segunda máquina qué se ve.

## Fase 7 — Práctica y análisis (MVP 2)

- [ ] Entrevistador simulado con dificultad, tipo, puesto, duración e idioma
- [ ] Respuesta por voz del usuario y transcripción
- [ ] Scoring: contenido, claridad, estructura, relevancia (§12)
- [ ] Detección de muletillas y duración media de respuesta
- [ ] Weakness Report acumulado entre sesiones (§13)
- [ ] Estadísticas por proyecto

## Fase 8 — Pulido (MVP 3)

- [ ] Benchmark de modelos dentro de la app
- [ ] RAG avanzado: reranking, búsqueda híbrida léxica + vectorial
- [ ] i18n completo, UI y entrevista en idiomas separados (§14)
- [ ] Instalador firmado
- [ ] Implementaciones reales de macOS y Linux

## Fuera de alcance por ahora

Fine-tuning, LoRA, análisis emocional, vídeo, lenguaje corporal, plugins y sincronización
entre dispositivos (§24). La arquitectura los admite; implementarlos ahora perjudicaría al
MVP.

## Política de tests

No hay tests de UI hasta la Fase 8. Sí desde el primer día en: chunking, retrieval y
umbrales, clasificador de preguntas, parseo de respuestas del LLM, migraciones de base de
datos y detección de hardware. Es la parte donde un fallo silencioso pasa desapercibido y
degrada las respuestas sin que nadie se entere.

Las migraciones se prueban **sobre una base que ya existe**, no solo creando una nueva: una
base en v2 con proyectos, documentos y trozos dentro tiene que llegar a v3 con todo, y
cualquier versión antigua tiene que acabar con el mismo esquema que una base recién creada.
Ese segundo test recorre todas las versiones, así que una migración futura que se olvide de
un índice lo rompe sin que nadie tenga que acordarse de ampliarlo. Los dos se comprobaron
rompiendo la migración a propósito antes de darlos por buenos: un test de migración que
nunca ha fallado no ha demostrado nada.
