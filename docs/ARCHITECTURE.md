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
transcripción empezaría a mitad de la primera sílaba.

**Lo que todavía no hace:** trocear turnos de más de 30 s, que es la ventana de whisper. Hoy
se recorta y se avisa en el log; la solución es la transcripción incremental sobre ventanas
solapadas, que es lo que además bajará la latencia percibida.

El idioma está fijado a español hasta que exista el selector de §14. Dejar que el modelo lo
detecte cuesta una pasada más y acierta peor con frases cortas, que es exactamente lo que
son los turnos de una entrevista.

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
