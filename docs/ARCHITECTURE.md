# Interview Copilot — Arquitectura

Documento vivo. Registra las decisiones tomadas y por qué. Si una decisión cambia, se reescribe aquí, no se añade debajo.

## 0. Restricción que condiciona todo el diseño

La máquina de desarrollo es un Lenovo 82VG: Ryzen 3 7320U (4c/8t Zen 2), 8 GB físicos de los que **5,74 GB son utilizables** (la iGPU reserva el resto), Radeon 610M con 2 CU y memoria compartida, sin CUDA ni ROCm en Windows.

Consecuencia, no estimada a ojo: la generación de tokens en CPU está limitada por ancho de banda de memoria (~30 GB/s reales con LPDDR5-5500 en bus de 64 bits). Un modelo de 3B en Q4_K_M ocupa ~2 GB, así que el techo teórico son ~15 tok/s y lo realista 8-12. El prefill del prompt de RAG en 4 núcleos Zen 2 va a 30-60 tok/s. Una respuesta corta sale en 15-40 s. El objetivo del producto son 2-4 s.

El cuello de botella real no es ni siquiera la velocidad: es la RAM. App + STT + embeddings + LLM 3B suman unos 3 GB, y el sistema en reposo ya consume 4-5 GB de los 5,74. El modelo acabaría en el pagefile.

**Por tanto la RAM es el criterio de desempate en todas las decisiones de stack de abajo, y la capa de providers de §18 del spec deja de ser una elegancia arquitectónica para convertirse en el mecanismo que hace la app usable en este hardware.**

Esto no cambia el default del spec: el modo por defecto sigue siendo LOCAL. Lo que hace el detector de hardware es decir la verdad al usuario y recomendar el modo que su máquina puede sostener.

## 0.1 El presupuesto de memoria, medido con todo dentro (2026-08-19)

Hasta la Fase 4 cada pieza se había medido por separado y el total se sumaba a mano. Con
las tres ya existiendo, se pueden cargar juntas en un proceso y mirar lo que pesan de
verdad (`hardware/budget.rs`):

| Paso | Residente | Incremento |
|---|---|---|
| Proceso vacío | 15 MB | — |
| **+ embeddings (`multilingual-e5-base`)** | **1.750 MB** | **+1.735 MB** |
| + una consulta | 1.753 MB | +3 MB |
| + VAD (Silero) | 1.769 MB | +16 MB |
| + whisper `base` | 1.914 MB | +145 MB |
| tras transcribir | 1.680 MB | −234 MB (Windows recorta) |

**El modelo de embeddings es el 90% del presupuesto.** §2.1 hablaba de "1 GB en disco" y de
compensarlo cargándolo solo durante el indexado; el coste real en memoria es 1,7 GB, casi el
doble, y el VAD y whisper juntos son 161 MB, o sea ruido a su lado.

Consecuencias, y ninguna es cosmética:

1. **Soltar el modelo de embeddings tras indexar deja de ser una elegancia y pasa a ser
   obligatorio.** Ya está implementado (`release_embedder`) y ahora se sabe cuánto vale.
2. **Durante la entrevista hay que volver a cargarlo**, porque la pregunta se embebe en
   vivo, así que el suelo del pipeline en vivo son ~1,9 GB más la propia aplicación (261 MB
   en desarrollo). En una máquina con 5,74 GB útiles y una videollamada abierta, eso es
   justo. Falta medirlo con Meet corriendo, que es la prueba que importa.
3. **Cambiar de modelo de embeddings a uno pequeño no es una opción**, y esto ya se midió:
   `e5-small` acierta 2 de 6 preguntas frente a 6 de 6 (§2.1). Se paga la memoria o se
   pierde la recuperación.

El LLM local sumaría ~2 GB más, lo que confirma por tercera vez y ahora con cifras que en
esta máquina el modo LOCAL en vivo no existe: el detector recomienda HYBRID y tiene razón.

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

## 3.2 Los datos de contacto no entran en el índice

§31 pide no pasear datos personales por la pantalla, y hasta ahora la cabecera del CV
—teléfono, correo, perfiles— se troceaba como un fragmento más. El coste no es teórico:
son cinco fragmentos los que se le mandan al modelo en cada pregunta, y uno de ellos se
lo llevaba un dato que no responde a ninguna entrevista.

Se limpia en `rag/contact.rs`, **antes de trocear y sobre las líneas del documento**. Después
de `chunking::normalize` los saltos simples ya son espacios, y para entonces el teléfono y
la primera sección del CV son el mismo párrafo. El documento se guarda entero: lo que se
recorta es lo que se indexa, que es lo único que puede acabar en pantalla y en el prompt.

La regla se escribió dos veces, y las dos correcciones salieron de medir contra el CV real
en vez de razonar sobre un CV imaginario:

1. **Primera versión: tirar la línea entera si era solo contacto.** Sobre el CV real quitó
   exactamente una línea, el teléfono, y dejó el correo dentro. `pdf-extract` había puesto
   nombre, puesto, correo y ciudad en la misma línea, así que la línea tenía contenido de
   sobra y se salvaba con el correo dentro.
2. **Segunda versión: quitar el dato, no la línea** —y tirar la línea solo cuando lo que
   queda no llega a cuatro palabras. Seguía sin funcionar: el extractor parte el correo por
   el espacio y devuelve `usuario @dominio.com`, dos piezas de las que ninguna es un correo
   por separado.

La versión que sí funciona reconoce las dos mitades y las junta. Medido sobre el CV real:
dos datos fuera (correo y teléfono), 8 fragmentos antes y 8 después, y el primero deja de
contener datos de contacto. El número se enseña en la UI al terminar de indexar, igual que
las citas descartadas de §5: es el único dato con el que juzgar si el filtro quita de más.

**Lo que no detecta, y por eso esto no es un anonimizador:** un nombre suelto en su línea.
Ninguna regla mecánica separa "SANTIAGO URBANEJA" de "EXPERIENCIA LABORAL" —las dos son
cortas, en mayúsculas y sin punto final— e intentarlo se llevaría por delante los
encabezados de sección, que son la frontera semántica más fuerte de un CV. Tampoco una
dirección postal sin etiqueta. Lo que hace es sacar del índice lo que una máquina reconoce
sin margen de duda: correos, teléfonos de nueve dígitos o más y URLs de perfil.

La distinción entre `github.com/santiago` y `github.com/santiago/proyecto` está puesta a
propósito: el primero es un dato de contacto y el segundo es un proyecto del candidato,
que es justo lo que hay que indexar.

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

## 4.1 Captura de audio: dos restricciones que dan forma al módulo

**`cpal::Stream` no es `Send`.** En Windows la sesión de audio pertenece al hilo que la
abrió, así que el flujo no puede guardarse en el estado de Tauri, que se comparte entre
hilos. Vive en un hilo propio que lo crea y se queda bloqueado en un `recv()` hasta que se
suelta el emisor. Parar la captura es soltar la estructura, no apagar un interruptor: eso
garantiza que el dispositivo queda libre antes de intentar abrir otro, que es justo lo que
falla al cambiar de micrófono si se hace al revés.

**La llamada de retorno de audio no puede bloquearse.** Se ejecuta con un plazo de
milisegundos, y un mutex, una reserva de memoria o un `log::info!` ahí no producen un
retraso de dibujo sino un corte de audio. Por eso lo único que hace es medir la ventana y
escribir dos `f32` en atómicos, con el buffer de conversión reutilizado entre llamadas.

De ahí salen dos decisiones más:

- **El nivel no viaja por un `Channel` de Tauri**, al revés que la respuesta del LLM. La
  llamada de retorno entra cada 10 ms: serían cien mensajes por segundo para una barra que
  se repinta sesenta veces. La UI pregunta cuando va a dibujar.
- **El pico se retiene hasta que se lee.** La UI mira cada 100 ms y una ventana de audio
  dura 10: sin retención, nueve de cada diez picos no se verían y una saturación breve
  pasaría desapercibida, que es exactamente lo que un medidor tiene que enseñar.

**El audio del sistema no es otro módulo.** WASAPI graba lo que suena abriendo un
dispositivo de *salida* en modo captura, y cpal pone el flag de loopback por su cuenta al
construir un flujo de entrada sobre un `eRender`. Separar por fuente —micrófono para el
usuario, loopback para el entrevistador— es además cómo se distingue quién habla en el MVP 1,
sin reconocer voces. Con altavoces en vez de auriculares esa separación deja de funcionar, y
la UI lo dice.

Hay una trampa medida el 2026-08-19: **en silencio, el loopback no entrega ni una muestra**,
porque WASAPI solo produce datos mientras la salida está activa. Se resuelve manteniendo
abierto un flujo de reproducción mudo sobre el mismo dispositivo; con él, un segundo de
silencio entrega las 96 000 muestras que le tocan. Importa más de lo que parece: sin eso, el
hueco en el flujo tiene el tamaño exacto de las pausas, y la pausa es la señal con la que la
Fase 5 detecta el fin de turno.

El suelo del medidor es −100 dB y no −∞ por un motivo que no es estético: JSON no tiene
infinito, `serde_json` lo serializa como `null`, y al otro lado hay un `number`. Hay un
test que lo fija.

## 4.2 El VAD, y las 64 muestras que costaron una tarde

El disparador de toda la entrevista es saber cuándo el entrevistador ha terminado de
preguntar. Un umbral de decibelios no sirve, y no es cuestión de afinarlo: el ruido de una
sala lo cruza y una pausa para pensar no. Silero mira la forma de la señal, no su energía.

El módulo está partido en dos a propósito. `Silero` habla con el modelo y solo se puede
probar con el ONNX de verdad; `TurnDetector` decide, a partir de la probabilidad, cuándo
empieza y acaba un turno, y es una máquina de estados que se prueba con números escritos a
mano. La parte que puede equivocarse en silencio —confundir una pausa con el final de una
pregunta— es la segunda, y es la que tiene tests.

La histéresis es lo que hace que una pregunta con pausas siga siendo **una** pregunta: dos
ventanas de 32 ms con voz abren turno, y hacen falta 700 ms de silencio para cerrarlo. Los
tres números (0,5 de umbral, 2 ventanas, 700 ms) son los de referencia de Silero o razonados
sobre cómo habla la gente, **no medidos con entrevistas reales**, y así está anotado en el
código. Por eso la UI enseña la probabilidad y su máximo: son los números con los que
calibrarlos el día que haya grabaciones de verdad.

### El fallo que enseñó a no fiarse de que "funcione"

La primera versión pasaba ventanas de 512 muestras al modelo. No daba ningún error —la
entrada del ONNX es dinámica— y devolvía probabilidades plausibles. Con una frase hablada
sin nada por delante daba 0,54, justo por encima del umbral: parecía funcionar y estaba mal.

Con **un segundo de silencio delante**, la misma frase daba 0,10. Un segundo de silencio es
exactamente lo que hay entre dos preguntas de una entrevista, así que en la app real el
detector no habría detectado nada, y el síntoma habría sido "el VAD no va" sin nada a lo que
agarrarse.

La v5 de Silero no recibe 512 muestras: recibe **576**, las 512 de la ventana más 64 de
contexto de la ventana anterior. Con el contexto puesto, esa misma frase da 0,98 con y sin
silencio delante, y el turno se cierra limpio.

Tres cosas que dejó por escrito ese rato:

1. **Una probabilidad plausible no es una probabilidad correcta.** El 0,54 pasaba el umbral
   y habría pasado una revisión a ojo.
2. **El camino sospechoso hay que medirlo, no razonarlo.** Se llegó comparando el mismo
   audio por dos caminos —fichero y captura en vivo— y viendo que daban 0,54 y 0,003.
3. **Un arreglo puesto sobre un diagnóstico equivocado hace daño.** Antes de encontrarlo se
   añadió una política de reiniciar el estado del modelo tras un rato de silencio; no
   arreglaba nada y cada reinicio metía un transitorio que abría turnos que nadie había
   hablado. Se quitó al arreglar la causa.

El test que lo protege mide la misma frase con 0, 1 y 5 segundos de silencio delante, y
exige más de 0,9 en las tres.

### La cadena entera, probada sola

Hay un test que hace hablar al sintetizador de voz de Windows, lo captura por el loopback,
lo remuestrea a 16 kHz y comprueba que Silero ve un turno de la duración correcta. Es lo más
cerca de una entrevista que se puede estar sin un entrevistador delante, y es automático.
Detecta lo que los tests de cada pieza por separado no pueden: que el remuestreo esté
desalineado, que la cola no se llene o que el VAD no vea el audio que sí está entrando.

El audio va de la llamada de retorno al VAD por una cola sin bloqueos (`ringbuf`), y se
remuestrea a 16 kHz mono antes de encolarlo: convertirlo una vez en el productor es más
barato que mandar el triple de datos y tirarlos al otro lado. Si la cola se llena se
descartan las muestras nuevas **y se cuentan**, porque un detector que no vio parte del
audio no puede presentarse como si lo hubiera visto todo.

## 4.3 Transcripción: whisper.cpp, y lo que cuesta de verdad en esta máquina

whisper.cpp compilado dentro del binario, vía `whisper-rs`. Es la pieza que decidía si el
modo LOCAL sirve para una entrevista o solo para practicar, y por primera vez hay números:

| Medida | Valor (2026-08-19, `whisper-base`) |
|---|---|
| Cargar el modelo | 0,55 s |
| Transcribir 3,7 s de voz | 2,0 s |
| Cadena entera: voz → loopback → VAD → texto | 1,9 s tras cerrarse el turno |

Es **0,54× tiempo real** con el modelo `base` en 3 hilos, y el texto salió palabra por
palabra correcto. Muy por debajo de lo que se temía en §0, donde la preocupación era si
esta máquina daría para transcribir en vivo. Da.

Tres decisiones de diseño, y ninguna es de estilo:

- **La transcripción va por turnos, no en continuo.** El VAD cierra un turno y ese audio se
  manda a transcribir. Es lo que permite que el texto salga completo y con puntuación en vez
  de a trozos, y encaja con lo que hace falta después: la pregunta entera es lo que dispara
  la recuperación y la respuesta (§10).
- **Va en su propio hilo, detrás de un canal.** Transcribir tarda segundos; si el hilo del
  VAD esperase, la siguiente pregunta se perdería entera. Si se acumulan turnos, se cuentan
  y se enseñan: una cola que crece significa que el equipo no da abasto y hay que bajar de
  modelo.
- **whisper se queda con un núcleo menos de los que hay.** Con los cuatro, la captura de
  audio da cortes, y el audio perdido no se recupera después; el texto sí puede tardar.

Hay un colchón de 256 ms antes de que el turno se dé por empezado (`PREROLL_FRAMES`): abrir
turno exige dos ventanas con voz, y para cuando se abre esas dos ya han pasado. Sin él, la
transcripción empezaría a mitad de la primera sílaba. **Medido el 2026-08-21 en §4.5: hacen
falta entre 8 y 54 ms, asi que sobra por cinco.**

**Lo que todavía no hace:** trocear turnos de más de 30 s, que es la ventana de whisper. Hoy
se recorta y se avisa en el log; la solución es la transcripción incremental sobre ventanas
solapadas, que es lo que además bajará la latencia percibida.

El idioma está fijado a español hasta que exista el selector de §14. Dejar que el modelo lo
detecte cuesta una pasada más y acierta peor con frases cortas, que es exactamente lo que
son los turnos de una entrevista.

## 4.4 Medir la calidad de una transcripcion: WER, y por que no un parecido (2026-08-21)

Hasta ahora "se entiende bien" era una opinion. `stt/wer.rs` la convierte en un numero, y
`stt/benchmark.rs` lo usa para comparar configuraciones sobre el mismo audio.

**Por que WER y no una libreria de similitud difusa.** La tentacion era `thefuzz` o
equivalente: un porcentaje de parecido y listo. No sirve, y no por ser de Python —que
tambien— sino porque **mezcla los tres errores en uno**. Los tres fallos que se guardaron
como respuestas de entrenamiento el 2026-08-21 son de tipos distintos y llevan a sitios
opuestos:

| Lo que se guardo | Que error es | A donde lleva |
|---|---|---|
| "Santiago y tengo 21 años" por "Me llamo Santiago…" | dos **borrados** | el audio no llego: es el VAD, no el modelo |
| "[Música]" sobre un turno de 64 ms | la referencia entera perdida | turno espurio: sobra el turno, no el modelo |
| "¡Aguien es bien!" | **sustituciones** | aqui si: modelo o senal de entrada |

Cero sustituciones con dos borrados dice que el modelo entendio perfectamente lo que le
llego. Un parecido del 85% no distingue ese caso del contrario, y elegir mal cuesta el dia.

Las reglas de comparacion son las mismas dos indulgencias de §5, por el mismo motivo:
mayusculas y puntuacion no cuentan —whisper puntua a su manera y penalizarlo seria medir su
estilo—, **los acentos si**, porque en español "años" y "anos" son palabras distintas.

### Lo medido, y el control que lo hace creible

Seis frases del dominio del CV real, dichas por el sintetizador de Windows a 16 kHz mono y
metidas directamente a whisper, sin microfono ni VAD de por medio:

| Configuracion | WER | S | B | I | x tiempo real |
|---|---|---|---|---|---|
| `base`, como esta hoy | 0,089 | 4 | 3 | 1 | 0,40 |
| `base` + `suppress_nst` | 0,089 | 4 | 3 | 1 | 0,38 |
| `base` + la pregunta como contexto | 0,089 | 4 | 3 | 1 | 0,37 |
| `base` + las dos | 0,089 | 4 | 3 | 1 | 0,37 |
| **CONTROL: prompt con faltas** | **0,122** | **7** | 3 | 1 | 0,38 |

**Ninguna de las dos palancas baratas mejora nada.** Y la fila del control es la que da
derecho a afirmarlo: un contexto inicial lleno de faltas deliberadas ("PIKING",
"KARRETILLERO") empeora el WER de 0,089 a 0,122, lo que demuestra que el prompt **si llega
al decodificador**. Sin esa fila, "el prompt no cambia nada" y "el prompt no se esta
aplicando" se leerian igual en la tabla, y son conclusiones opuestas.

Que `suppress_nst` no cambie nada aqui era de esperar y no lo descarta: sobre voz limpia no
hay tokens de no-habla que suprimir. Su caso es el turno de 64 ms, que este banco **no puede
producir** porque entra por el VAD, no por el modelo.

De paso queda medido otro numero: `base` sobre voz sintetica limpia da **0,089 de WER**. Las
respuestas reales de Santiago salieron incomparablemente peor ("¡Aguien es bien!"). Esa
distancia no la explica el modelo —es el mismo—, asi que esta en el camino del audio: nivel
de entrada, microfono o VAD. Es el siguiente sitio donde mirar, y no el tamaño del modelo.

### Lo que el banco no puede ver

- **El principio comido.** El audio entra por fichero, asi que el recorte del VAD no existe
  aqui. Un WER bueno en esta tabla no dice que la cadena funcione: dice que el modelo
  entiende lo que le llega.
- **La voz real.** El sintetizador habla mas limpio que nadie por un micro de auriculares.
  Los numeros ordenan configuraciones entre si; no predicen el acierto sobre una persona.

## 4.5 Que el colchon de arranque no era el culpable (2026-08-21)

§4.4 dejo la pregunta abierta: `base` da 0,089 de WER sobre voz limpia y las respuestas
reales salieron incomparablemente peor, asi que el fallo esta en el camino del audio y no en
el modelo. La sospecha nombrada era `PREROLL_FRAMES`, los 256 ms de colchon que se guardan
antes de dar el turno por empezado, anotados en §4.3 como "de sobra y practicamente gratis"
sin haberlo medido nunca.

`audio/benchmark.rs` lo mide. Compara dos instantes sobre el mismo audio: donde empieza la
senal de verdad —por energia, en ventanas de 5 ms, y no con el propio VAD, que daria cero
por construccion— y donde `TurnDetector` abre turno. La diferencia es **el colchon que haria
falta para no perder nada**.

Las seis frases son las mismas de §4.4, y eso no es comodidad: los dos bancos miden la misma
cadena rota por sitios distintos, y comparar numeros sacados de audio distinto no diria nada.
Por eso el sintetizador, el lector de WAV y el corpus se mudaron a `testing.rs`; llegaron a
estar escritos tres veces, y tres copias de un instrumento son tres instrumentos en cuanto se
toca una.

### Lo medido

| Condicion | Abre | Colchon max | Colchon medio | Perdido con los 256 ms de hoy |
|---|---|---|---|---|
| volumen entero | 6/6 | 54 ms | 35 ms | **0** |
| a la mitad | 6/6 | 54 ms | 40 ms | **0** |
| a la cuarta parte | 6/6 | 54 ms | 40 ms | **0** |
| a la decima parte | 6/6 | 54 ms | 40 ms | **0** |
| CONTROL: 1 s de silencio delante | 6/6 | 50 ms | 37 ms | **0** |

**`PREROLL_FRAMES` no se queda corto: sobra por cinco.** Hacen falta entre 8 y 54 ms y hay
256. La hipotesis que llevaba abierta desde el 2026-08-21 por la manana queda descartada, y
con ella la tentacion de subir la constante, que habria costado el dia sin tocar la causa.

**Y la ganancia no mueve nada.** A la decima parte de volumen Silero sigue dando 1,000 de
probabilidad y abre el turno en el mismo sitio. Un microfono flojo, por si solo, no produce
un principio comido. Eso deja el fallo (a) —400 a 700 ms perdidos del arranque— sin
explicacion en el VAD, y el siguiente sitio donde mirar es **el tiempo que tarda el
dispositivo en entregar la primera muestra despues de abrirlo**: si el modo diapositiva abre
el microfono y ensena la pregunta a la vez, lo que se habla mientras Windows abre la sesion
de audio no es que se descarte, es que no existe. Los 400-700 ms encajan con eso mucho mejor
que con un colchon de 256. **Medido en §4.6: son 343 ms de mediana, y 250 de ellos son
el modelo del VAD cargandose antes de abrir el dispositivo.**

### Los tres controles, y por que hacen falta aqui mas que nunca

Esta tabla es un **resultado nulo por partida doble** —ni el colchon ni la ganancia cambian
nada— y un resultado nulo sin control no se distingue de un banco desenchufado:

1. **Un segundo de ceros delante.** El instante en que abre se corre un segundo entero y el
   colchon necesario no se mueve mas de una ventana. Si se moviera, la tabla estaria midiendo
   el relleno en vez del arranque.
2. **El detector de energia no mira el volumen.** El arranque se busca contra el pico del
   propio fichero, asi que sale identico a las cuatro ganancias — y el test lo exige. Si
   cambiara con la ganancia, la columna del colchon estaria midiendo el detector de energia y
   no el VAD, que es justo la conclusion contraria.
3. **Tres segundos de ceros no abren ningun turno.** Si lo abrieran, ninguna fila de arriba
   significaria nada.

### El turno corto, que es el otro banco

El fallo (b) es un turno espurio de 64 ms al abrir el microfono, que produjo `[Música]` y se
guardo solo. Sesenta y cuatro milisegundos son exactamente las dos ventanas que exige
`FRAMES_TO_START`. Para poder tirar ese turno hay que saber donde esta el suelo de una
respuesta corta legitima, porque tirar tambien los "si" seria cambiar un fallo por otro:

| Palabra | Voz del turno |
|---|---|
| "No." | **256 ms** |
| "Sí." | 288 ms |
| "Ya." | 288 ms |
| "Vale." | 352 ms |
| "Ajá." | 352 ms |
| "Correcto." | 544 ms |

**El turno legitimo mas corto dura 256 ms y el transitorio duro 64: hay 192 ms de margen.**
Cabe de sobra una duracion minima de turno que separe las dos cosas sin tocar ni el umbral ni
la histeresis, que es la parte del VAD que si esta calibrada contra algo.

**Puesta el 2026-08-22: `MIN_TURN_MS` son 128 ms**, y el numero no se elige, sale de los dos
extremos. Es la media **geometrica** de 64 y 256: el doble del transitorio y la mitad del
turno mas corto, o sea igual de lejos de ambos medido en veces y no en milisegundos. Eso es
lo que corresponde cuando los dos extremos son inciertos —del transitorio hay una sola
observacion y el "No." lo dijo un sintetizador—, porque maximiza el margen por el lado por el
que se acabe fallando. Y cae en cuatro ventanas exactas, que no es coincidencia agradable
sino requisito: la duracion se cuenta en ventanas de 32 ms, asi que un umbral entre dos de
ellas seria en realidad el de al lado con otro nombre.

Justo en el umbral el turno **se conserva**. Ante la duda pesa mas la respuesta corta del
candidato: transcribir un chasquido de mas se ve en pantalla, y tirar un "si" de verdad no.

Un turno por debajo del umbral no desaparece, **se cuenta**: `TurnDiscarded` es un evento
propio y `VadState::discarded` lo lleva a la pantalla. Es la misma regla que las muestras que
se pierden cuando la cola se llena — un turno tirado en silencio es indistinguible de un
turno que nunca ocurrio, y uno al abrir el microfono es el transitorio y esta bien, mientras
que muchos seguidos significan que algo mete ruido en la entrada.

Que el umbral caiga entre los dos numeros medidos **se comprueba al compilar**, no en un
test: es una condicion entre constantes, y moverlo fuera del hueco tiene que romper la
compilacion en vez de un test que alguien podria no llegar a correr. Comprobado bajandolo a
64 ms a proposito: la compilacion falla con el motivo escrito.

### Lo que estos bancos no pueden ver

- **El transitorio de apertura.** Lo produce el dispositivo al abrirse, no un fichero, asi
  que aqui no existe. Del transitorio real hay **una sola observacion**, la del 2026-08-21, y
  una observacion no es una distribucion: el suelo de 256 ms se puede defender, el techo de
  64 no. La otra mitad de la fila la tiene que traer un volcado de turnos cortos desde la app.
- **El suelo de ruido.** Bajar la ganancia de un fichero limpio baja tambien su ruido, y un
  microfono flojo de verdad no hace eso: la voz baja y el ruido de sala se queda donde estaba.
  La fila "a la decima parte" descarta que el volumen por si solo retrase el arranque; no
  descarta que una relacion senal-ruido mala lo haga.
- **La voz real**, igual que en §4.4. El sintetizador habla mas limpio que nadie.

## 4.6 La ventana muerta del arranque: el modelo del VAD se carga en cada apertura (2026-08-22)

§4.5 descarto el colchon y dejo una hipotesis: lo que se pierde del principio no se descarta,
**no existe**, porque se habla mientras el dispositivo todavia se esta abriendo. Aqui esta
medida, y la respuesta es mas concreta de lo que apuntaba la hipotesis.

`Meter` sella dos instantes desde que se pide la captura: cuando el dispositivo dice estar
abierto y **cuando llega la primera ventana de audio de verdad**. El segundo es el que
cuenta; un dispositivo puede dar la sesion por abierta y tardar despues en entregar nada. Los
dos salen en `CaptureStatus`, asi que dejan de ser un numero de banco y pasan a ser
diagnostico: "el microfono no me oye" y "el microfono tarda medio segundo en encenderse" se
parecen en pantalla y llevan a sitios opuestos.

| Condicion | Primera muestra (mediana) | Min | Max |
|---|---|---|---|
| primera apertura del proceso, sin VAD | 267 ms | — | — |
| en caliente, sondeando cada 1 ms | 92 ms | 84 | 105 |
| en caliente, sondeando cada 50 ms | 94 ms | 89 | 117 |
| **en caliente, con el VAD cargando** | **343 ms** | **264** | **544** |

**Abrir el dispositivo cuesta unos 90 ms. Cargar Silero antes de abrirlo cuesta 250 mas.**
`Recorder::start` construye el `VoiceTracker` —y con el lee el ONNX del disco— **antes** de
tocar la tarjeta, a proposito: asi un modelo corrupto es un error al pulsar "Escuchar" y no un
hilo que muere en silencio. El precio de esa decision no se habia medido nunca, y es que cada
apertura del microfono arrastra una carga de modelo entera.

En el modo diapositiva eso ocurre **una vez por pregunta**: el `useEffect` que abre el
microfono depende del indice, asi que veinte preguntas son veinte cargas de Silero. Sumando la
ida y vuelta por IPC y el renderizado, la ventana muerta real esta por encima del medio
segundo, que es justo el orden de los 400-700 ms que faltaban en las respuestas guardadas.

**El colchon no podia arreglarlo, y ahora se ve por que.** `PREROLL_FRAMES` guarda audio ya
capturado; aqui no hay audio que guardar. Subirlo a un segundo no habria cambiado una palabra.

### El control, y lo que descubrio antes de pasar

El riesgo de este banco es cronometrar al que mira: si el instante se tomase en el bucle de
espera, sondear cada 50 ms daria numeros hasta 50 ms mas altos que sondear cada 1 ms. La marca
se pone **dentro** de la llamada de retorno de audio, asi que las dos columnas tienen que
coincidir, y el test lo exige.

La primera version del banco **fallo ese control**: 113 ms contra 83. No era el sondeo. Era
que la primera apertura del proceso costaba 339 ms y caia siempre en la primera columna, con
lo que dos efectos distintos estaban sumados en una sola cifra. Separada la apertura en frio,
el control pasa con 5 ms de diferencia entre las dos medianas. Es el mismo tipo de hallazgo
que el prompt con faltas de §4.4, solo que al reves: alli el control confirmo una conclusion,
aqui la impidio.

Una precaucion mas, pequena y de la misma familia: la marca de la primera muestra se pone al
**final** de `Meter::push`, despues de contar las muestras. Al principio habia un instante en
que el estado decia "ya llego la primera muestra" con el contador todavia a cero, y un
diagnostico que se contradice a si mismo no sirve para diagnosticar nada.

### El arreglo: la sesion se comparte, el estado no

`VadModel` es el modelo ya cargado, y vive en `AppState` como el provider de LLM y por el
mismo motivo. `Recorder::start` lo recibe cargado en vez de recibir una ruta.

Lo que se comparte es la **sesion de ONNX Runtime**, no el detector. Una sesion de ORT es
`Send + Sync` y la inferencia no la muta —el estado recurrente de la v5 viaja como tensor de
entrada y de salida—, asi que el microfono y el loopback pueden usar la misma a la vez.
Cada captura se construye su `Silero`, con el estado a cero. Compartir tambien el estado
haria que el primer turno de una fuente arrastrase el final de la otra, y ese es
exactamente el tipo de fallo que §4.2 describe: no da error, da probabilidades plausibles.

Dos cosas que no cambian, a proposito:

- **El fallo sigue apareciendo al pulsar "Escuchar".** El modelo se carga bajo demanda, no al
  arrancar, y una carga fallida no se guarda en la cache. Un ONNX corrupto sigue siendo un
  error visible de "arrancar la captura" y no un hilo que muere en silencio, que era la razon
  de cargarlo donde se cargaba.
- **Sin modelo descargado la captura arranca igual**, sin deteccion de voz. Obligar a bajar
  2,3 MB antes de poder ver el medidor seria poner una puerta donde no hace falta.

| Condicion | Ventana muerta (mediana) | Min | Max |
|---|---|---|---|
| primera apertura del proceso | 202 ms | — | — |
| en caliente, sin VAD | 74 ms | 70 | 83 |
| **en caliente, VAD compartido (hoy)** | **72 ms** | 69 | 86 |
| en caliente, VAD recargado (antes) | 183 ms | 178 | 217 |

**111 ms menos por apertura**, y la fila de abajo es la que da derecho a decirlo: es el
comportamiento viejo medido en la misma pasada y en la misma maquina, no un numero recordado
de antes del cambio.

El ahorro depende de lo caliente que este ORT. Medido el 2026-08-22 en dos pasadas: **250 ms
la primera vez que se midio** —cuando la sesion del banco era la primera del proceso— y 111
ms una vez el motor y la cache de disco estan calientes. La primera apertura de todas costo
544 ms. En la app la secuencia real es la mala: el usuario abre el microfono en la primera
pregunta, que es la fria.

### Lo que sigue pendiente de aqui

- **No abrir el microfono a la vez que se ensena la pregunta.** Aunque la carga ya no este,
  quedan los ~72 ms del dispositivo, y el modo diapositiva no tiene por que regalarlos: puede
  abrirlo mientras se dibuja la pregunta anterior.
- **`firstSampleMs` a la vista en Ajustes → Microfono.** El numero ya viaja en el estado; un
  micrófono que tarda un segundo en arrancar es un problema del equipo del usuario, y sin
  ensenarlo se diagnostica como un fallo de la aplicacion.

## 4.7 Transcribir cuesta lo mismo dure lo que dure, y eso tumba la mejora que estaba planeada (2026-08-22)

Queja de campo, no de banco: *"tarda bastante en transcribir"*. La Fase 4 tenia pendiente
justo para esto la **transcripcion incremental sobre ventanas solapadas**, apuntada desde §4
como una de las cuatro optimizaciones del diseno. Antes de construirla habia que saber **de
que depende** el rato, porque las dos posibilidades llevan a soluciones opuestas.

| Audio | Duracion | Coste | x tiempo real |
|---|---|---|---|
| 1 frase | 5,3 s | 2426 ms | 0,46 |
| 2 frases | 10,2 s | 2558 ms | 0,25 |
| 3 frases | 15,8 s | 2841 ms | 0,18 |
| 4 frases | 21,5 s | 3591 ms | 0,17 |
| 5 frases | 26,4 s | 3824 ms | 0,15 |
| **CONTROL: las 5 por separado** | 26,4 s | **11 438 ms** | 0,43 |

**El audio se multiplica por cinco y el coste por 1,6.** whisper rellena su entrada hasta 30 s
de mel siempre, asi que lo que se paga es la ventana, no la voz. El coste no es una tasa: es
un **suelo de 2,4 a 3,8 s por turno**, casi plano.

**Y el control lo convierte en una decision.** Trocear ese mismo audio en cinco pasadas cuesta
**3 veces mas** que una sola: 11,4 s contra 3,8. La transcripcion incremental, tal y como esta
escrita en el roadmap, no repartiria el coste — lo multiplicaria, y encima para recortar una
espera que ya esta dominada por un suelo fijo. **Queda descartada en esta maquina y con este
modelo**, y el sitio donde estaba apuntada como mejora pasa a decirlo.

De paso corrige un numero que se venia arrastrando. §4.3 y §4.4 dan "0,37-0,45x tiempo real",
y es cierto — sobre frases de cinco segundos. Como **tasa** es enganoso: predice 12 s para un
turno de 30 y la medicion da 3,8. La forma util de decirlo es "entre 2,4 y 3,8 s por turno,
casi independientemente de lo que dure".

El tope de 30 s tampoco es teorico: la sexta frase del corpus no cabe y el propio proveedor la
rechaza, que es como aparecio en esta tabla.

### Donde esta de verdad la espera del modo diapositiva

Sumando lo que hay entre dejar de hablar y que la respuesta se guarde:

| Etapa | Cuanto |
|---|---|
| Cierre de turno del VAD | 0,7 s |
| whisper | 2,4 - 3,8 s |
| Sondeo de la UI | hasta 0,4 s |
| **Cuenta atras antes de guardar** | **4 s** |

**La pieza mas grande no era el modelo, era la cuenta atras** — y estaba puesta sobre una
estimacion. El comentario que la justificaba decia "lo que tarda whisper (~2 s aqui)" y
buscaba un total de "unos seis segundos callado". Con 3 s reales de whisper el total eran
ocho, no seis.

Asi que la cuenta baja a **2 s**, que es lo que devuelve el total a los ~6,1 s que la
constante siempre quiso valer. No es un numero mas corto porque si: es el mismo calculo con
la medicion que faltaba cuando se escribio.

Bajarla solo es seguro por un segundo cambio, y este si es un arreglo: **volver a hablar
cancela la cuenta en cuanto el VAD oye voz**, sin esperar al texto. Antes solo la cancelaba el
texto nuevo, y el texto tarda tres segundos y medio: quien retomaba la frase despues de pensar
se encontraba con que la pantalla ya habia guardado y pasado de pregunta. Ese fallo ya existia
con los cuatro segundos; acortar la cuenta sin arreglarlo lo habria hecho frecuente.

### Y lo que se veia y donde

El coste por turno se medía desde el 2026-08-19 y solo se enseñaba en Ajustes → Audio, que es
justo donde no esta quien espera. Igual que el contador de turnos descartados de §4.5, que se
estreno el 22-08 en la pantalla equivocada y por eso el primer informe de campo sobre el dijo
"no aparecio ninguno" sin que se pudiera saber si es que no habia. Los dos estan ahora en el
modo diapositiva. **Un numero que solo se ve donde no se necesita es un numero que no existe.**

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

Esto dejó de ser una hipótesis el 2026-08-19. Con `llama3.2:1b` sobre el CV real, ante "cuéntame un proyecto complicado":

```json
"citations": [
  {"fragment": "Maquinaria y Equipos", "quote": "Carnet de Carretillero"},
  {"fragment": "Competencias Transversales", "quote": "Capacidad de trabajo físico pesado"}
],
"answer": "…asistente de gestión logística en una empresa de construcción… 5000 toneladas
           de materiales, incluyendo hormigón, acero y plástico…"
```

La respuesta es ficción entera: ni el puesto, ni la empresa, ni las cifras están en el CV. **Las dos citas literales sí lo están.** Se descartó, pero por el motivo equivocado: porque el modelo escribió el título de la sección donde el prompt pide el número, no porque se detectara la invención. Con `"fragment": 1` habría pasado el filtro.

Conclusión que hay que tener presente antes de fiarse de esta capa: la cita literal demuestra que el modelo **leyó** los documentos, no que la respuesta **salga** de ellos. Es una barrera contra el modelo que se inventa el respaldo, no contra el que se inventa la historia y adorna con una frase real.

La siguiente barrera candidata, sin implementar y sin medir: exigir que toda cifra que aparezca en la respuesta aparezca también en los fragmentos citados. Habría cazado las "5000 toneladas". Es estrecho y mecánico, que es lo que aquí funciona; pero antes de ponerlo hay que medir cuántas respuestas buenas rechaza, igual que se hizo con el umbral de similitud y con el modelo de embeddings.

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

## 5.1 El entrenamiento previo: de dónde sale el material que la IA no puede inventar

§5 pide preparar a la IA antes de la entrevista y §6 prohíbe que invente experiencia. Las
dos frases son la misma cosa vista por los dos lados, y hasta la Fase 4 solo estaba
implementado el lado prohibitivo: la cita verificada, que —medido— demuestra que el modelo
leyó los documentos y no que la respuesta salga de ellos.

**La garantía real no es un filtro más estricto: es tener material de verdad en el momento
de la pregunta.** Y el material lo pone el candidato antes, contestando las preguntas que le
van a hacer. Eso es el panel de entrenamiento.

### Tres decisiones de diseño

**Las respuestas no cuelgan de un proyecto.** Hasta v3 todo documento pertenecía a una
entrevista, así que cada oferta nueva empezaba de cero. Desde v4, `project_id` admite NULL y
NULL significa "es del candidato": el CV, las respuestas entrenadas y sus historias valen
para todas las entrevistas, y el proyecto solo aporta la oferta concreta. Es lo que hace que
esto mejore con el uso en vez de repetirse en cada entrevista.

**Se guarda la pregunta junto a la respuesta**, en un solo párrafo, y cada trozo indexado
lleva la pregunta delante. Las dos cosas salieron de medir, no de razonar:

- Con la pregunta dentro, el parecido se mide **entre preguntas**, que es la tarea para la
  que E5 está entrenado (§2.1) y donde de verdad funciona.
- Con la pregunta y la respuesta en párrafos distintos, el troceador las separaba y el mejor
  resultado de la búsqueda acababa siendo el fragmento que solo contiene la pregunta:
  recuperaba de maravilla (0,93) y era inútil, porque al modelo le llegaba la pregunta que ya
  sabía y no la respuesta que hacía falta.

**Se contesta hablando o escribiendo.** Dictar no es un lujo: la respuesta dictada suena a
cómo habla el candidato, y es esa forma de decirlo la que hace que la sugerencia en vivo
suene humana en vez de a currículum leído. Además se contesta en un minuto lo que escribiendo
cuesta cinco, y un banco de respuestas a medias no sirve de nada. Reutiliza entero el camino
de la Fase 4: micrófono, VAD y whisper.

### Lo que esto cambia, medido

Con el modelo real, ante *"cuéntame una vez que tuviste un conflicto con un compañero"*:

| Fragmento | Similitud |
|---|---|
| **La respuesta entrenada** | **0,8746** |
| CV — competencias transversales | 0,7960 |
| CV — experiencia laboral | 0,7614 |

La respuesta entrenada gana por casi nueve centésimas, que para los márgenes de este corpus
(§2.1 medía diferencias de 0,003 a 0,04) es una distancia enorme.

Y hay un segundo efecto que no es evidente: **la cita literal deja de estorbar**. Citar
palabra por palabra un CV telegráfico escrito en tercera persona es difícil, y por eso el
filtro de §5 rechazaba respuestas buenas; citar la respuesta que el propio candidato dictó,
no. El filtro pasa de ser una barrera a ser un seguro barato.

Lo que **no** cambia: el modelo sigue pudiendo adornar lo que se le da. La cita verificada se
queda como suelo. Lo que cambia es que ahora hay suelo y hay material.

## 5.2 La oferta de empleo no es experiencia del candidato (medido el 2026-08-20)

El retriever ordena solo por similitud y el origen de cada fragmento no pinta nada en esa
decisión, aunque `DocumentKind` esté ahí desde la Fase 2 con un comentario que promete
usarlo "para pesar la recuperación más adelante". Este es ese más adelante, y lo primero
fue medir si hacía falta.

Corpus: el CV real y una oferta del puesto que persigue. Preguntas: el banco entero de
entrenamiento, las veinte, sin elegir (`rag/indexer.rs`, test
`de_donde_salen_los_cinco_fragmentos_que_ve_el_modelo`).

| Medida | Resultado |
|---|---|
| Preguntas con material de la empresa en el top 5 | **19 de 20** |
| Fragmentos de oferta sobre los 100 sitios disponibles | **36** |
| Preguntas donde la oferta es el **primer** resultado | **12 de 20** |
| Margen entre el mejor de la oferta y el mejor del CV | −0,0330 a +0,0091 |

**Por qué es grave y no una imprecisión.** Ante *"cuéntame un proyecto complicado en el que
hayas trabajado"*, el mejor fragmento que recibe el modelo es la oferta: un documento que
dice lo que la empresa **pide**, no lo que el candidato **hizo**. Y la barrera de §5 lo deja
pasar, porque esa frase sí está literalmente en los documentos indexados. Es el mismo camino
por el que `llama3.2:1b` se inventó un puesto entero adornado con dos citas reales del CV,
solo que aquí el material inventable se lo sirve el propio retriever.

Los márgenes son minúsculos, como en §2.1. La oferta no gana por ser más relevante: gana
porque **repite el vocabulario del CV** —"picking y packing", "carnet de carretillero",
"control de stock", "trabajo en equipo"—. Los embeddings miden de qué habla un texto, no de
quién habla, y eso ya está medido en §5: es la misma limitación que tumbó el umbral.

**La conclusión no es la que se esperaba, y por eso se midió antes de escribir el código.**
El plan era un peso por origen: una constante que hiciera valer menos a la oferta. No sirve,
porque la oferta no vale menos siempre. Vale menos **según la pregunta**, y el reparto sigue
exactamente al `QuestionKind` que el banco ya lleva:

- `Behavioral`, `Experience`, `SelfAssessment` — la oferta no aporta nada y contamina 9 de
  las 13. Son las preguntas de "cuéntame una vez que...", donde la respuesta tiene que salir
  de la experiencia del candidato o no salir.
- `Motivation`, `Logistics` — la oferta es justo el material bueno. Para *"¿por qué quieres
  trabajar aquí?"* (3 de 5) o *"¿cuál es tu disponibilidad?"* contestar sin leerla sería el
  error contrario.

Así que no es un multiplicador que calibrar sino **un filtro por tipo de pregunta**, con un
`kind` que ya existe en los dos extremos: en el banco durante el entrenamiento, y en el
clasificador de §7 durante la entrevista. Una constante menos que inventar, que es la
diferencia entre una regla que se puede defender y un número puesto a ojo.

### El filtro, y lo que cambia con él puesto

`Material::{All, CandidateOnly}` en `rag/retriever.rs`. El mismo test recorre el banco dos
veces, con filtro y sin él, para que la mejora sea un número y no una promesa:

| | Preguntas contaminadas | Fragmentos de oferta | La oferta es la 1ª |
|---|---|---|---|
| Sin filtro | 19 de 20 | 36 de 100 | 12 |
| **Con filtro** | **0 de 20** | **0 de 100** | **0** |

Los 100 sitios siguen llenos, y eso también se comprueba: cuando el filtro deja el top 5 a
medias se le piden más vecinos al índice, doblando hasta llenarlo o hasta que el índice se
agote. Los huecos que libera la oferta tienen que ocuparlos fragmentos del candidato, que es
justo el punto; un top 5 que se queda en tres es media respuesta. Se dobla en vez de pedir
una cantidad fija porque no hay proporción de material de empresa que valga para todos los
corpus: depende de lo gorda que sea la oferta frente al CV.

El destaque (`standout`) se sigue midiendo sobre la primera ventana de candidatos y no se
toca. Si creciera con el pozo dejaría de ser comparable entre preguntas: es una media, y
ampliar la muestra la mueve sola.

Dónde se aplica hoy, y por qué no en todas partes:

- `AnswerStyle::Behavioral` y `Technical` → `CandidateOnly`. La técnica es la más peligrosa
  de las tres y por eso no se deja abierta: una oferta enumera justo las herramientas que
  pide, así que dejarla entrar es servirle al modelo la lista de lo que le conviene decir
  que sabe.
- `AnswerStyle::General` → `All`, como estaba. Es el cajón de sastre: ahí caen tanto
  "cuéntame sobre ti" como "¿por qué quieres trabajar aquí?", y esa segunda **necesita** la
  oferta. Elegir un lado sería adivinar cuál de las dos tenía el usuario en la cabeza, y no
  hay nada medido que lo diga. Lo resuelve el clasificador de §7.
- La búsqueda manual del índice sigue viendo **todo**. Existe para mirar con los ojos lo que
  hay dentro, así que recortarla la convertiría en otra cosa: quien la usa necesita ver
  también lo que el filtro deja fuera al contestar.

Lo que **no** mide este test, y hay que decirlo: el peso de las respuestas entrenadas frente
al CV. Eso sigue con el único dato de §5.1 —0,8746 contra 0,7960— y una sola pregunta.

## 5.3 Lo que se guarda solo hay que verlo antes de que se guarde (2026-08-22)

El modo diapositiva quito friccion y de paso quito la ultima oportunidad de mirar lo que se
estaba archivando: cualquier texto en la caja disparaba la cuenta de 4 s y se guardaba. El
2026-08-21 se archivaron asi ocho respuestas inservibles, y no por descuido — la pantalla las
estaba ensenando. **Verlo pasar no es verlo.**

Importa mas de lo que parece porque una respuesta entrenada le gana la recuperacion al CV por
nueve centesimas (§5.1): lo que entra mal aqui se queda arriba del todo en todas las
entrevistas siguientes.

**Lo que esto no es.** No puntua respuestas ni decide si una respuesta es buena. Eso necesita
un modelo capaz, que en esta maquina no hay, y ponerle un porcentaje seria el error que §12
ya tiene anotado. Lo unico que decide es **si la respuesta se guarda sola o hay que mirarla**,
y la asimetria manda: un falso positivo cuesta un clic, un falso negativo cuesta el corpus.

Tampoco es "confirmar todo". Confirmar veinte respuestas seria devolver las veinte decisiones
que este modo existe para quitar, y a los tres avisos nadie los lee. Una respuesta normal se
sigue guardando sola.

### Las cuatro reglas, y de que corpus salen

De comparar dos corpus reales, no de imaginar como falla una transcripcion: **las ocho
respuestas envenenadas tal cual se guardaron**, que estan en el test y se quedan ahi para
siempre, contra **las seis frases del corpus de referencia** (§4.4), que es como suena una
respuesta correcta del mismo dominio.

| Regla | De donde sale |
|---|---|
| Marca de no-habla | Se buscan **los corchetes**, no una lista de palabras: la lista se queda corta el dia que el modelo escriba `[Ruido]` en vez de `[Música]`, y nadie dicta corchetes. `♪` aparte, que whisper la usa suelta |
| Menos de **10 palabras** | La respuesta correcta mas corta del corpus tiene 13; la envenenada mas larga de las cazables por longitud, 8. Diez es la media geometrica, igual que `MIN_TURN_MS` y por lo mismo |
| Empieza a media frase | Minuscula inicial **o** conjuncion. Solo la minuscula no bastaba: "Y de ultimo lejos se voy…" lleva mayuscula porque whisper la pone al empezar aunque lo que oyo empezara a la mitad |
| Guiones de dialogo | whisper los mete cuando cree oir a dos personas, y una respuesta dictada es una sola |

### Lo medido

**Siete de las ocho envenenadas se cazan. Ninguna de las seis buenas.**

La segunda cifra es el control y es la que decide si esto vale: una regla que marca
respuestas validas no es una regla, es ruido, y un aviso que salta siempre deja de existir
aunque siga en el codigo.

La que se escapa esta escrita aparte en el test, con nombre propio: *"Ah, y me voy a estar a
ver de ahi, un boque este video."* Tiene largo de respuesta, empieza en mayuscula y no lleva
ninguna marca. Para verla hace falta entender lo que dice, y eso es otro problema. Ponerla
como limite explicito en vez de como fila que falta es lo que evita creerse que el filtro
caza todo.

### Lo que el umbral de palabras todavia no sabe

En el corpus de referencia **no hay ni una respuesta legitimamente corta**, porque no hay
preguntas de `Logistics`: "¿cual es tu disponibilidad?" se contesta de verdad en seis
palabras. Cuando las haya, el suelo hay que volver a medirlo. Mientras tanto el error cae del
lado barato — esa respuesta se marca y se guarda con un clic — y queda dicho aqui en vez de
descubrirse usando la aplicacion.

### Donde se aplica

Por el mismo camino pasan la cuenta atras **y el boton de guardar**. El boton tambien, porque
una respuesta envenenada guardada por un clic impaciente envenena igual, y la confirmacion es
una sola. Si la revision falla por lo que sea, tampoco se guarda: seguir a ciegas porque la
comprobacion no contesto seria quitar justo la red que se acaba de poner.

`training/review.rs` es una funcion pura detras de un comando, no logica de pantalla. Va en
Rust y no en el frontend por dos motivos: aqui hay tests y en el frontend no (§Politica de
tests), y la Fase 7 pide lo mismo para Practica — que pregunta por cada respuesta si se
guarda como material — asi que reescribirlo alli seria tener dos filtros que se separan.

## 5.4 El clasificador de preguntas, y el "no se" que lo sostiene (2026-08-22)

§5.2 cerro `Behavioral` y `Technical` al material de la empresa y dejo `General` abierto
diciendo "lo resuelve el clasificador de §7". Este es ese clasificador.

Decide dos cosas en la entrevista: **con que material se contesta** y **con que forma** se
redacta la sugerencia. Vive junto a la taxonomia, en `training/classifier.rs`, porque lo que
produce es un `QuestionKind` y ese enum se define ahi.

### Reglas primero, y por que no el modelo

Es la decision del cuadro de riesgos: clasificar con el LLM anade **una pasada entera** al
camino critico, y la latencia es el punto mas delicado del producto (§10). Una pregunta de
entrevista es de las pocas cosas de este dominio con formulas fijas —"cuentame una vez que…",
"que harias si…", "cuales son tus expectativas salariales"—, asi que las reglas contestan la
mayoria en microsegundos.

**Y no hay ni un umbral que calibrar.** Cada patron que encaja suma un punto a su tipo; gana
el que mas tenga. Solo hay dos formas de quedarse sin respuesta, y ninguna lleva numero
dentro: que nadie puntue, o que haya empate arriba. Se eligio asi contra la alternativa
evidente, que era pesar los patrones — un peso es una constante puesta a ojo, y este proyecto
ya ha pagado dos veces por una. Cuando un patron generico se comia a uno especifico, la
solucion fue **estrechar el generico**: "cuentame un…" no dice nada, "cuentame una vez que…"
si.

Los acentos no cuentan, al reves que en el WER de §4.4. Alli "años" y "anos" son palabras
distintas y esa diferencia es justo lo que se mide; aqui el texto llega de whisper, que a
veces se los come, y ninguna de las seis clases depende de una tilde.

### Los tres corpus, y cual de ellos vale

| Corpus | Que es | Que demuestra |
|---|---|---|
| Las 20 de `QUESTIONS` | Desarrollo. Se mira mientras se escriben las reglas | Poco: acertar donde has mirado |
| `EVALUACION`, 32 preguntas | **Sellado.** Escrito antes que las reglas, sin tocarlo despues | El numero que se publica |
| `SIN_TIPO`, 7 frases | **Control.** Cosas que se dicen en una entrevista y no son ninguna de las seis | Que el "no se" existe |

Lo del corpus sellado es la leccion que dejo Trading Lab: un conjunto de evaluacion que se
ajusta hasta que el resultado gusta ha dejado de medir y se ha convertido en la
implementacion escrita dos veces.

### Lo medido

| Corpus | Bien | Equivocadas | Al modelo |
|---|---|---|---|
| Banco (desarrollo) | 19/20 | **0** | 1 |
| **Sellado** | **31/32** | **0** | 1 |
| Control | — | **0 de 7 reciben tipo** | 7 |

**96% de las preguntas se resuelven sin modelo y ninguna sale con el tipo equivocado.**

Las dos cifras no valen lo mismo y el test lo dice: la abstencion tiene salida —la resuelve
el LLM— y una clasificacion equivocada no la tiene, porque cambia el material con el que se
contesta sin que nadie se entere. Por eso lo que se exige es cero equivocadas, no un
porcentaje bonito.

**El control es el que sostiene la arquitectura entera.** Si "¿me oyes bien?" recibiera
etiqueta, "reglas primero y el LLM solo ante ambiguedad" seria una frase vacia: no habria
ambiguedad que detectar, habria un valor por defecto con nombre de decision.

### La unica que no se clasifica, y por que es la respuesta correcta

*"¿De que logro profesional estas mas orgulloso?"*. El banco la tiene como `Experience` desde
el 19-08. Al escribir el corpus sellado, sin mirar el banco, la casi identica *"¿De que
trabajo te sientes mas orgulloso?"* salio etiquetada como `SelfAssessment`.

**Dos personas etiquetando lo mismo de dos formas distintas es la definicion operativa de
ambiguo.** Asi que la regla se quito y las dos van al modelo. Inventar un desempate habria
sido decidir a ojo lo que ni el propio proyecto tiene decidido, y habria salido 20/20 y 32/32
en las tablas de arriba: mejor numero, peor sistema.

Se toco el corpus sellado una sola vez y queda escrito en el fichero: "¿Tienes alguna pregunta
para nosotros?" estaba en el control, y el banco la tiene como `Motivation` desde el 19-08. El
banco es anterior, asi que la equivocada era la lista. Es una correccion contra una autoridad
externa y previa, no un ajuste para mejorar un numero.

### Que cambia en la recuperacion

`material_for` deja de mirar solo el estilo y consulta al clasificador cuando el estilo es
`General`. El reparto por tipo sale de §5.2 para cinco de los seis; `Situational` **esta
razonado y no medido** —el banco solo trae dos preguntas de ese tipo— y queda dicho igual que
`DocumentKind::Other`: "¿que harias si…" pregunta por el criterio del candidato, y una oferta
que enumera lo que la empresa espera es la lista de lo que conviene contestar.

**Y cuando el clasificador no se moja, se cierra.** Es un cambio respecto a como estaba, y
sale de que los dos errores no cuestan lo mismo: cerrar de mas da una respuesta mas pobre y
**se ve**, porque el modelo dice que no tiene material; abrir de mas mete en el top 5 un
documento que dice lo que la empresa pide, la barrera de §5 lo deja pasar porque esa frase si
esta en los documentos, y sale experiencia inventada con cita real. Eso **no se ve**.

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
