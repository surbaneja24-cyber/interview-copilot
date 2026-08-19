# Interview Copilot — Arquitectura

Documento vivo. Registra las decisiones tomadas y por qué. Si una decisión cambia, se reescribe aquí, no se añade debajo.

## 0. Restricción que condiciona todo el diseño

La máquina de desarrollo es un Lenovo 82VG: Ryzen 3 7320U (4c/8t Zen 2), 8 GB físicos de los que **5,74 GB son utilizables** (la iGPU reserva el resto), Radeon 610M con 2 CU y memoria compartida, sin CUDA ni ROCm en Windows.

Consecuencia, no estimada a ojo: la generación de tokens en CPU está limitada por ancho de banda de memoria (~30 GB/s reales con LPDDR5-5500 en bus de 64 bits). Un modelo de 3B en Q4_K_M ocupa ~2 GB, así que el techo teórico son ~15 tok/s y lo realista 8-12. El prefill del prompt de RAG en 4 núcleos Zen 2 va a 30-60 tok/s. Una respuesta corta sale en 15-40 s. El objetivo del producto son 2-4 s.

El cuello de botella real no es ni siquiera la velocidad: es la RAM. App + STT + embeddings + LLM 3B suman unos 3 GB, y el sistema en reposo ya consume 4-5 GB de los 5,74. El modelo acabaría en el pagefile.

**Por tanto la RAM es el criterio de desempate en todas las decisiones de stack de abajo, y la capa de providers de §18 del spec deja de ser una elegancia arquitectónica para convertirse en el mecanismo que hace la app usable en este hardware.**

Esto no cambia el default del spec: el modo por defecto sigue siendo LOCAL. Lo que hace el detector de hardware es decir la verdad al usuario y recomendar el modo que su máquina puede sostener.

## 1. Perfiles de ejecución

| Modo | STT | LLM | Latencia esperada aquí | Uso |
|---|---|---|---|---|
| LOCAL | whisper.cpp local | llama.cpp local | 15-40 s | Práctica, o hardware potente |
| HYBRID | whisper.cpp local | API externa | 1,5-3 s | **Recomendado en esta máquina** |
| CLOUD | API externa | API externa | 2-4 s | Sin hardware suficiente |

En HYBRID el audio nunca sale del equipo: solo viaja el texto ya transcrito y recortado por el retriever. Es un punto intermedio real, no un lavado de cara, pero sigue siendo salida de datos y la UI lo declara según §15.

## 2. Stack — decisiones y alternativas descartadas

### Shell de escritorio: Tauri 2 (descartado Electron)

Electron parte de 350-500 MB de RAM residente; Tauri usa el WebView2 ya presente en el sistema. Sobre 5,74 GB utilizables esa diferencia es un porcentaje nada despreciable de toda la memoria de la máquina, todos los días, mientras el usuario está en una entrevista. Además Tauri da acceso nativo a Win32 sin addon de C++ (necesario para §26-28) y produce instaladores de ~10 MB en vez de ~150 MB.

Medido en la máquina de referencia con la app arrancada en modo desarrollo: 36 MB del proceso Rust más 225 MB en 6 procesos de WebView2, **261 MB en total**. Una build de release, sin devtools ni HMR, queda por debajo. Al medir hay que contar solo el árbol de procesos que cuelga de `interview-copilot.exe`: en este equipo hay otras aplicaciones usando WebView2 y sumarlas todas da una cifra tres veces mayor que la real.

Coste asumido: hay que instalar la toolchain de Rust + MSVC Build Tools + CMake, y la primera compilación es lenta. Medida real en 4 núcleos con `CARGO_BUILD_JOBS=2`: **9 minutos**. Es un coste de una sola vez, a cambio de un ahorro de memoria permanente.

### Frontend: React 19 + TypeScript estricto + Vite, CSS plano con variables

React porque es lo que el mantenedor está aprendiendo en el bootcamp: el código tiene que poder tocarlo él. `strict: true`, `noUncheckedIndexedAccess` y `exactOptionalPropertyTypes` desde el primer commit.

Sin Tailwind ni librería de componentes: la UI son cinco pantallas y §19 pide cambiar tema, opacidad y tamaño de fuente en caliente, lo que se resuelve escribiendo variables CSS en `:root` desde la capa de settings. Una dependencia menos que mantener y ningún paso de build extra.

### Motor: Rust dentro del propio proceso de Tauri (descartado sidecar de Python)

La opción obvia era un sidecar Python con faster-whisper y sentence-transformers: Python ya está instalado y el ecosistema de RAG es más cómodo. Se descarta por tres razones concretas:

1. Un intérprete de Python cargado con CTranslate2 y ONNX añade 300-400 MB residentes y un segundo proceso que gestionar, en la máquina donde la RAM es el recurso crítico.
2. El Python instalado es 3.14.5. CTranslate2 (base de faster-whisper) no publica wheels para 3.14, así que habría que instalar además un Python 3.12 paralelo.
3. Distribuir la app dejaría de ser un instalador y pasaría a ser "instala Python y estas dependencias", lo que choca con el objetivo de §17 (facilidad para distribuir).

Componentes en Rust:

| Función | Crate | Nota |
|---|---|---|
| Captura de audio | `cpal` | WASAPI, micrófono + loopback de sistema en Windows |
| VAD | Silero VAD vía ONNX | Detecta fin de turno del entrevistador |
| STT | `whisper-rs` (bindings de whisper.cpp) | Cuantizado, backend Vulkan opcional |
| Embeddings | `fastembed` | `multilingual-e5-base`, elegido por medición (ver §2.1) |
| Persistencia | `rusqlite` + extensión `sqlite-vec` | Un solo fichero .db, sin servidor |
| Win32 y DXGI | crate `windows` | Bindings oficiales de Microsoft |

Sobre la detección de GPU: se usa DXGI y no WMI porque reporta la memoria tal y como la ve
el runtime de gráficos, que es lo que de verdad limita a llama.cpp o whisper.cpp. Hay una
trampa medida en la máquina de referencia: la Radeon 610M **integrada** declara 2022 MB de
`DedicatedVideoMemory`, que son un recorte de los 8 GB del sistema y no memoria adicional.
DXGI no expone ningún campo que diga "soy integrada", así que la clasificación se hace por
tamaño (umbral de 3 GB) y no por la proporción entre memoria dedicada y compartida, porque
esa proporción tampoco distingue: una gráfica dedicada de 4 GB en un equipo de 32 GB también
declara más memoria compartida que dedicada.

### 2.1 Modelo de embeddings: elegido midiendo, no razonando

La primera elección fue `multilingual-e5-small` por argumentos que sonaban bien sobre el
papel: multilingüe, 384 dimensiones, pequeño. El banco de pruebas de
`src-tauri/src/embedding/benchmark.rs` —seis fragmentos de un CV en español y seis
preguntas de entrevista con la respuesta correcta conocida— lo desmintió:

| Modelo | Acierto top-1 | Margen medio | Tamaño |
|---|---|---|---|
| multilingual-e5-small | 2/6 | −0,0082 | 465 MB |
| multilingual-e5-small (sin prefijos) | 3/6 | −0,0023 | 465 MB |
| **multilingual-e5-base** | **6/6** | **+0,0157** | 1075 MB |
| paraphrase-multilingual-mpnet-base-v2 | 3/6 | −0,0013 | 1075 MB |
| paraphrase-multilingual-minilm-l12-v2-q | no infiere | — | 240 MB |

Tres conclusiones que valen más que la tabla:

1. **No es cuestión de tamaño.** mpnet-base pesa exactamente lo mismo que e5-base y
   acierta la mitad. Lo que decide es el objetivo de entrenamiento: E5 está entrenado para
   recuperación asimétrica (pregunta corta contra párrafo largo), que es literalmente esta
   tarea; mpnet para similitud entre frases parecidas, que no lo es.
2. **El modelo cuantizado no es una opción**, no por calidad sino porque el ONNX de Qdrant
   falla en inferencia con las versiones actuales de ONNX Runtime (`Missing Input:
   encoder.layer.0.attention.output.LayerNorm.weight`).
3. Antes de culpar al modelo se descartaron las tres explicaciones que habrían sido un
   fallo propio: que fastembed aplicara los prefijos por su cuenta y se estuvieran
   duplicando (no lo hace), que el *pooling* fuera el equivocado (usa media para E5, que es
   lo correcto) y que los vectores salieran degenerados (la matriz de similitud del corpus
   da dispersión 0,071 con diagonal exacta).

Coste asumido: 1 GB en disco y 768 dimensiones en vez de 384, lo que duplica el tamaño del
índice. Se compensa cargando el proveedor de embeddings solo durante el indexado y
liberándolo después, de modo que esa memoria no está ocupada durante la entrevista, que es
cuando el equipo va justo.

**Consecuencia para §6 (no inventar experiencia):** los márgenes son estrechos incluso con
el modelo bueno (+0,003 a +0,04 sobre una base de ~0,85). Un umbral absoluto de similitud
—"si el mejor resultado baja de 0,8, avisa de que no hay experiencia relevante"— sería
arbitrario con estas cifras. El umbral tiene que ser relativo: comparar el mejor resultado
contra la distribución del resto del corpus, no contra una constante.

### Base vectorial: sqlite-vec (descartadas Chroma y LanceDB)

Chroma arrastra Python. LanceDB añade un formato de fichero propio y decenas de MB de dependencias. sqlite-vec es una extensión de la SQLite que ya vamos a usar para todo lo demás: una sola base, un solo backup, un solo DELETE ALL DATA (§15). El corpus de una entrevista son cientos de chunks, no millones — no hace falta nada más pesado.

### LLM: siempre fuera de proceso, detrás de HTTP

Ni siquiera el proveedor local se enlaza dentro de la app: habla con un `llama-server` o con Ollama, que exponen API compatible con OpenAI. Ventajas: el usuario cambia de modelo sin reiniciar la app, un crash del modelo no se lleva la aplicación por delante, la RAM del modelo se libera sola, y todos los providers comparten el mismo cliente HTTP y el mismo código de streaming.

**Proveedor de nube elegido: OpenAI.** Era la única decisión que quedaba bloqueada esperando al usuario. Anthropic no se implementa: su API de mensajes tiene otra forma de petición y otro formato de streaming, así que sería código nuevo que no se puede probar contra nada, y código de red sin probar es una promesa, no una función.

**Una sola implementación, no una por proveedor.** §18 nombra `LocalLLMProvider` y `OpenAIProvider` como piezas separadas. No lo son: Ollama, `llama-server` y api.openai.com hablan el mismo protocolo byte a byte, así que dos structs serían dos copias del mismo código separadas por una URL y una cabecera — justo lo que prohíbe §23. Lo que sí está separado es lo único que difiere de verdad, y vive en `ProviderKind`: si el proveedor pide clave y si los datos salen del equipo. El día que entre un proveedor con otro protocolo tendrá su propio struct implementando el mismo trait, que es cuando la separación aporta algo.

Hay un tercer proveedor, `MockProvider`, que fabrica una respuesta con el formato de §8 a partir de los propios fragmentos recuperados, sin consultar ninguna IA. Sirve para recorrer la ruta completa sin instalar nada y para provocar el caso de §6 a voluntad. Va detrás de `#[cfg(debug_assertions)]`: un proveedor que devuelve texto plausible sin haber consultado nada no puede existir en la versión que alguien se lleva a una entrevista.

### Claves de API: en el almacén del sistema, nunca en la base de datos

Las claves van al Administrador de credenciales de Windows, no al fichero SQLite. Ese fichero se copia, se puede adjuntar en un informe de errores y se borra entero con el botón de §15; una clave que viaje por cualquiera de esos tres caminos es una clave filtrada.

La regla que lo sostiene no es la elección del almacén sino esta: **no existe ningún comando que devuelva una clave al frontend.** §31 pide que no se muestren claves en la interfaz, y la única forma de garantizarlo es que el frontend no tenga por dónde pedirlas. Puede poner una, borrarla, o preguntar si hay alguna configurada. Nada más. El borrado total de §15 también las limpia: decir "borra todos mis datos" y dejar la clave guardada sería mentir en el botón.

## 3. Módulos

```
interview-copilot/
├── src/                      React + TS (UI)
│   ├── views/                Projects · Prepare · Interview · Practice · Settings
│   ├── components/
│   └── ipc/                  Wrappers tipados de los comandos Tauri
├── src-tauri/
│   ├── src/
│   │   ├── audio/            Captura, mezcla de fuentes, VAD, medidor de nivel
│   │   ├── stt/              Trait STTProvider + LocalWhisper + Cloud
│   │   ├── llm/              Trait LLMProvider + Local + OpenAI + Anthropic
│   │   ├── embedding/        Trait EmbeddingProvider + fastembed local
│   │   ├── rag/              Chunking, indexado, retriever híbrido
│   │   ├── interview/        Máquina de estados de la entrevista en vivo
│   │   ├── question/         Clasificador de intención (§7)
│   │   ├── practice/         Entrevistador simulado + scoring (§12-13)
│   │   ├── storage/          SQLite, migraciones, borrado total
│   │   ├── hardware/         Detección de CPU/RAM/VRAM y recomendación (§4)
│   │   └── platform/
│   │       ├── windows/      WindowCaptureProtection, loopback, hardware
│   │       ├── macos/        stub que devuelve Unsupported
│   │       └── linux/        stub que devuelve Unsupported
└── docs/
```

Regla: `platform/` es el único sitio donde se permite código específico de un sistema operativo, y se selecciona con `#[cfg(target_os = ...)]`. Ningún módulo de arriba llama a Win32 directamente.

## 3.1 Troceado: por qué el solape no cruza párrafos

El solape entre trozos consecutivos es doctrina estándar en RAG, y aplicado sin criterio hace daño. Con solape indiscriminado, un CV de tres secciones producía trozos que eran *el final de «lideré una migración»* pegado al *principio de «di clases de matemáticas»*. Cada trozo mezclaba dos experiencias sin relación, y ninguna de las dos se recuperaba limpia: la pregunta sobre enseñanza devolvía el fragmento que hablaba de microservicios.

El solape existe para no partir una idea continua por la mitad. Entre dos secciones distintas de un CV no hay ninguna idea que proteger, solo ruido que añadir. Por eso hay dos reglas:

- **El solape no cruza fronteras de párrafo.** Solo se aplica cuando un párrafo tuvo que partirse por dentro.
- **Un trozo solo abarca varios párrafos mientras siga siendo corto** (por debajo de 300 caracteres). Así una lista de viñetas se agrupa en algo con contexto suficiente, pero dos secciones sustanciales nunca acaban juntas.

Ambas salieron de un test de extremo a extremo con el modelo real, no de razonar sobre el papel. Los tests de troceado con datos inventados pasaban perfectamente mientras el defecto estaba ahí.

## 4. El pipeline en vivo, y dónde se gana la latencia

```
mic/loopback ─► VAD ─► ventana de audio ─► STT streaming ─► ¿fin de pregunta?
                                                                  │ sí
   respuesta en pantalla ◄── LLM (streaming) ◄── prompt ◄── retriever (top-k)
```

Cuatro optimizaciones que están en el diseño desde el principio, no como mejora posterior:

1. **Prefijo de prompt cacheado.** El system prompt y el perfil del candidato son idénticos durante toda la entrevista. Se envían una vez y se reutiliza el KV cache; solo se procesan de nuevo los chunks recuperados y la pregunta. Es la diferencia entre 1200 y 400 tokens de prefill.
2. **Embeddings precalculados.** Todo el corpus se indexa al crear el proyecto, nunca durante la entrevista.
3. **Transcripción incremental.** Whisper corre sobre ventanas solapadas mientras el entrevistador sigue hablando; al detectar fin de turno, la transcripción ya está casi completa.
4. **Retrieval especulativo.** En cuanto el VAD marca una pausa larga se lanza el retriever con la transcripción parcial, sin esperar a la confirmación de fin de turno. Si la pregunta continúa, se descarta y se repite: cuesta milisegundos de CPU y ahorra cientos de ms de latencia percibida.

## 5. La regla de no inventar experiencia (§6)

Es un requisito de producto, así que se implementa como control explícito y no como una frase en el prompt. Lo que sigue documenta **un intento fallido y la solución que lo sustituye**, porque el intento fallido es justo el que la intuición vuelve a sugerir.

### Lo que no funciona: un umbral sobre la similitud

La idea natural es: si el mejor resultado del retriever no llega a cierta puntuación, avisar de que no hay experiencia relevante. Se midió sobre un corpus con seis preguntas que sí tienen respuesta y cuatro que no (`embedding/benchmark.rs`, tests `calibra_el_umbral_*`):

| Señal | Preguntas con respuesta | Preguntas sin respuesta | ¿Separa? |
|---|---|---|---|
| Despegue sobre la media del corpus | 0,0185 – 0,0488 | 0,0109 – 0,0269 | no |
| Similitud absoluta del mejor | 0,7874 – 0,8557 | 0,8013 – 0,8350 | no |
| Reranker cross-encoder bge-v2-m3 | −11,01 – −2,76 | hasta −9,92 | no |

Las tres nubes se solapan. No es cuestión de afinar el número: la similitud entre embeddings mide **de qué habla** cada texto, no si uno responde al otro. Una pregunta sobre dirigir un equipo de ventas se parece muchísimo a un CV lleno de liderazgo y equipos aunque no contenga una sola línea sobre ventas. Y "¿cuál es tu mayor fracaso profesional?", que sí tiene respuesta en el corpus, puntúa **más bajo** que esa pregunta de ventas que no la tiene.

De paso quedó descartado el reranker: además de no separar, ordena peor que el bi-encoder (5/6 y 4/6 frente a 6/6), así que costaría un modelo de 1-3 GB y una pasada extra de latencia a cambio de nada medible.

Salvedad: el corpus de calibración son 6 fragmentos. El test queda para repetirlo con un CV real, pero el diseño no puede apoyarse en que los números mejoren solos.

### Lo que sí es verificable: citas literales

El aviso vive en la capa de generación, donde hay algo que una máquina puede comprobar. Los fragmentos se le presentan al modelo numerados `[1]`..`[k]`, numeración local a esa petición, y tiene que devolver dos cosas por cada afirmación: **qué fragmento la respalda** y **un trozo copiado palabra por palabra de ese fragmento**.

La segunda condición es la que aguanta el peso, y la primera versión de este diseño no la tenía. Comprobar solo que el número de fragmento exista es casi decorativo: con cinco fragmentos numerados, un modelo que se invente una experiencia escribe igualmente `"fragment": 1` y la cita "existe". Exigir una copia literal es otra cosa — para pasar el filtro, el modelo tiene que haber copiado palabras que de verdad están en los documentos del candidato.

Se numeran 1..k y no con los identificadores de la base a propósito. Un `rowid` de SQLite es un número de cuatro o cinco cifras, distinto en cada equipo, que un modelo pequeño copia mal a menudo; además saldría del equipo en el modo nube sin aportar nada. Con 1..k cualquier número fuera de rango es inequívocamente una referencia inventada.

Los dos modos de fallo no se tratan igual, y la asimetría es deliberada:

- **Un fragmento que nunca se envió tumba la respuesta entera.** No es un desliz de redacción: es el modelo fabricando respaldo, aunque las demás citas fueran impecables.
- **Una cita que no aparece literalmente se cae ella sola.** Casi siempre es una paráfrasis, que es un defecto mucho más benigno. Si no sobrevive ninguna cita, no hay respuesta.

"Literal" se entiende con dos indulgencias y solo dos: mayúsculas y espacios no cuentan (un salto de línea del PDF convertido en espacio no es una invención), y los puntos suspensivos parten la cita en trozos que deben aparecer en orden. Los acentos y la puntuación sí cuentan: en cuanto se empiezan a ignorar, "literal" pasa a significar "aproximadamente", que es el camino que ya se demostró que no lleva a ninguna parte.

**Lo que esto no garantiza,** y conviene tenerlo escrito para no venderlo de más: que el fragmento citado respalde *lo que la respuesta afirma*. Un modelo podría copiar una frase real del CV y colgarle al lado una afirmación inventada. Contra eso no hay comprobación mecánica; lo que hay es que la UI enseña la cita junto a la respuesta para que el candidato lo vea de un vistazo.

Un prompt que dice "no inventes" es una petición. Un umbral de similitud parecía una garantía y la medición demuestra que no lo es. Una cita literal verificada contra el texto enviado sí lo es: falla en cerrado, porque sin cita válida no hay respuesta que mostrar.

### Enseñar sin adelantarse: por qué las citas van primero en el JSON

§10 pide que la respuesta aparezca según se escribe. §6 prohíbe enseñar una experiencia que no esté respaldada. Parecen incompatibles: para verificar hace falta la respuesta entera, y para que haya streaming hay que empezar a enseñarla antes de tenerla.

Se resuelven con el orden de los campos. El prompt exige este orden exacto, y un JSON se genera de arriba abajo:

```
{"citations": [...], "answerable": ..., "answer": "...", "keyPoints": [...], "followUps": [...]}
```

Cuando empieza a llegar el texto de `answer`, las citas ya están completas y ya se han verificado. El extractor incremental (`llm/answer.rs`) va sacando el valor de `answer` carácter a carácter del JSON a medio escribir; la compuerta (`llm/answering.rs`) solo deja pasar esos caracteres si la verificación pasó. Si el modelo se salta el orden pedido, el texto se retiene en memoria hasta que lleguen las citas, y se tira sin haber salido a pantalla si no valen.

El resultado es que **nunca se muestra texto que luego haya que retirar**. Enseñar una experiencia inventada durante dos segundos y sustituirla después por un aviso sería peor que no enseñar nada: el candidato ya la ha leído.

Al terminar el stream se vuelve a parsear la respuesta completa y se verifica otra vez. El veredicto del streaming solo decide si se puede ir mostrando; el final es el que manda. Hay un test que comprueba que los dos coinciden, porque el día que dejen de hacerlo el síntoma sería texto que aparece y desaparece.

## 6. Protección de captura (§26-33)

Interfaz `CaptureProtection` con `enable` / `disable` / `is_supported` / `get_status`, y tres modos: `OFF`, `EXCLUDE_FROM_CAPTURE`, `MONITOR_ONLY`.

Desviación consciente del spec: §28 pide `native/windows/WindowCaptureProtection.cpp` con CMake. Se implementa en Rust con el crate `windows`, que son los bindings oficiales de Microsoft a la misma API `SetWindowDisplayAffinity`. Se llama exactamente a la misma función del sistema, con el mismo `GetLastError()`; lo que se evita es arrastrar un proyecto de C++ y una toolchain adicional para envolver una única llamada. §28 pide no depender de hacks de terceros si existe una API oficial, y esto la usa directamente.

Límite que la documentación interna y la UI deben respetar siempre: `WDA_EXCLUDEFROMCAPTURE` actúa sobre los mecanismos de captura que respetan la política de Windows. No impide una foto del monitor ni cualquier método que ignore esa política. La app nunca se describe como indetectable, sino como "window capture exclusion where supported".

## 7. Riesgos abiertos

| Riesgo | Impacto | Mitigación |
|---|---|---|
| LLM local inusable en esta máquina | Alto, ya confirmado | HYBRID recomendado por el detector de hardware |
| Compilar whisper.cpp con 4 núcleos y 5,7 GB | Medio | Una sola vez |
| El loopback WASAPI depende de la app de videollamada | Medio | Selector de dispositivo + medidor de nivel visible |
| Diarización: distinguir entrevistador de usuario | Medio | MVP1 separa por fuente (mic vs loopback), no por voz |
| Clasificar preguntas con LLM añade una pasada | Medio | Primera pasada por reglas; LLM solo si hay ambigüedad |
| Whisper con acento y ruido de sala | Medio | `base` como mínimo; subir a `small` si el hardware aguanta |
