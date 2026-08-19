# Interview Copilot

Copiloto de entrevistas de escritorio, local-first. Indexa tu CV, la oferta y tus notas;
escucha la entrevista, detecta las preguntas y sugiere qué responder a partir de tu
experiencia real.

## Estado

**Fases 0 a 3 implementadas.** Se puede crear un proyecto, cargar documentos, indexarlos y
escribir una pregunta a mano para recibir una respuesta con las fuentes citadas.

Lo que **no** existe todavía, por si el resto del texto sugiere lo contrario:

| | |
|---|---|
| Captura de audio y transcripción | Fase 4, sin empezar |
| Entrevista en vivo | Fase 5 |
| Protección de ventana ante compartir pantalla | Fase 6 |
| Modo práctica y análisis | Fase 7 |

Es decir: hoy la pregunta se escribe, no se escucha.

- `docs/ARCHITECTURE.md` — decisiones, con sus mediciones y las alternativas descartadas
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
arranca el frontend con `npm run dev`, que no tiene backend y por tanto no hace nada útil.

Para generar respuestas hace falta además un proveedor de LLM, que se elige en Ajustes:

- **Local** — Ollama o `llama-server` corriendo en tu equipo. No sale nada del ordenador.
- **OpenAI** — salen la pregunta y los fragmentos recuperados, nada más. La interfaz lo
  declara en cada respuesta.

## Cómo evita inventar experiencia

Es la regla que condiciona el diseño, así que conviene explicar qué hace y qué no.

El modelo tiene que devolver, junto a la respuesta, **un trozo copiado palabra por palabra**
de los fragmentos que se le enviaron. La aplicación lo comprueba contra el texto original.
Sin al menos una cita literal verificada no se muestra respuesta: se muestra un aviso y un
esqueleto de cómo estructurarla, escrito por la aplicación y no por el modelo.

La comprobación ocurre **antes** de enseñar nada. La respuesta aparece en pantalla según se
escribe, pero el prompt obliga a que las citas vengan primero en el JSON, así que cuando
empieza a llegar el texto ya se sabe si vale. Nunca se muestra algo que luego haya que
retirar.

Lo que esto **no** garantiza: que el fragmento citado respalde lo que la respuesta afirma.
Un modelo puede copiar una frase real de tu CV y colgarle al lado una historia inventada;
está medido y documentado en `ARCHITECTURE.md` §5. La cita demuestra que el modelo leyó tus
documentos, no que la respuesta salga de ellos. Por eso la interfaz enseña siempre la cita
al lado de la respuesta.

Un intento anterior —descartar la respuesta cuando la similitud del buscador bajase de
cierto umbral— se midió y no funciona. No se reabre sin volver a medir.

## Privacidad

Tus datos —CV, audio, transcripciones, respuestas— se guardan solo en tu equipo, en un
único fichero SQLite. Las claves de API no van ahí: van al almacén de credenciales del
sistema, y no existe ningún comando que las devuelva a la interfaz.

Ajustes tiene un botón que borra todo lo local, incluidas las claves guardadas.
