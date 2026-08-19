# Setup del entorno de desarrollo

Estado detectado en la máquina de desarrollo (Lenovo 82VG, Windows 11 build 26200):

| Herramienta | Estado |
|---|---|
| Node 24.13.0 | instalado |
| npm 11.6.2 | instalado |
| git 2.53.0 | instalado |
| ffmpeg 8.1 | instalado |
| WebView2 | instalado (viene con Windows 11) |
| Python 3.14.5 | instalado, **no se usa** en este proyecto |
| Rust | **falta** |
| MSVC Build Tools (workload C++) | **falta** |
| CMake | **falta** |

## Qué falta instalar y por qué

**MSVC Build Tools con el workload de C++.** Rust en Windows usa el linker de Microsoft,
así que hace falta aunque no escribamos C++. Ocupa unos 7 GB.

**Rust.** Es el lenguaje del backend de Tauri. Unos 1,5 GB.

**CMake.** Lo necesita whisper.cpp, que se compila desde fuentes al construir `whisper-rs`.

Total: unos 9 GB de los 66,8 GB libres.

## Instalación

Con winget, en una terminal normal (winget pide elevación por sí solo cuando la necesita):

```
winget install --id Microsoft.VisualStudio.2022.BuildTools --override "--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended" --accept-package-agreements --accept-source-agreements
```

```
winget install --id Rustlang.Rustup --accept-package-agreements --accept-source-agreements
```

```
winget install --id Kitware.CMake --accept-package-agreements --accept-source-agreements
```

Después cierra y vuelve a abrir la terminal para que el PATH se actualice, y comprueba:

```
rustc --version; cargo --version; cmake --version
```

## Arranque del proyecto

```
npm install
```

```
npm run tauri dev
```

La primera compilación tardó **9 minutos** en esta máquina (4 núcleos, `CARGO_BUILD_JOBS=2`).
Las siguientes son incrementales y bajan a segundos o pocos minutos. La cifra subirá cuando
entre whisper.cpp en la Fase 4, que se compila desde fuentes.

## Nota sobre memoria durante la compilación

Con 5,74 GB utilizables, enlazar el binario de Tauri y compilar whisper.cpp a la vez puede
agotar la RAM. Si `npm run tauri dev` falla con un error de memoria o el linker muere,
limita los trabajos paralelos de cargo:

```
$env:CARGO_BUILD_JOBS = "2"
```

Y cierra el navegador y VS Code mientras compila.

## Modelos locales

La app descarga sus propios modelos de STT y embeddings a `%APPDATA%/interview-copilot/models`.
No hay que instalarlos a mano.

Para el LLM local (opcional en esta máquina, ver `ARCHITECTURE.md` §0), hace falta un
servidor compatible con la API de OpenAI corriendo aparte:

```
winget install --id Ollama.Ollama
```

```
ollama pull qwen2.5:3b-instruct-q4_K_M
```

La app se conecta a `http://localhost:11434`. En esta máquina concreta ese modelo responde
en 15-40 s, útil para el modo práctica pero no para una entrevista en vivo.
