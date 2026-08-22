//! Las veintiuna respuestas que Santiago dicto de verdad el 2026-08-22.
//!
//! **Es el unico corpus de este proyecto que no ha escrito nadie para medir nada.** Son las
//! respuestas de una tanda entera de entrenamiento, copiadas tal cual quedaron en la base,
//! con la app en el estado en que estaba esa tarde: el arranque ya no se comia y whisper
//! acertaba, por su cuenta, "un 85%".
//!
//! Vale por dos motivos que ningun banco sintetico puede dar:
//!
//! - **Asi habla una persona**, no el sintetizador de Windows. Empieza con "Bueno,", se
//!   repite, se corrige a media frase y deja alguna concordancia por el camino.
//! - **Asi se rompe la cadena hoy**, que no es como se rompia ayer. Las ocho del 21-08 eran
//!   basura de transcripcion; estas tres estan cortadas o les falta el principio, que es un
//!   fallo distinto y se caza con otras senales.
//!
//! Etiquetadas a mano leyendolas una a una. `USABLE` no quiere decir perfecta —"una vez
//! cometio un error" deberia ser "cometi", "he trobarios proyectos" no es nada— sino que la
//! respuesta esta entera y dice lo que el candidato queria decir. Lo que se decide con esto
//! es si se archiva sola o si hay que mirarla, no si esta bien escrita.

#![cfg(test)]

/// Si la respuesta sirve como material del candidato o hay que pararse a mirarla.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Estado {
    Usable,
    Rota,
}

pub const RESPUESTAS: &[(Estado, &str)] = &[
    // --- Las tres rotas ---------------------------------------------------------------
    //
    // Cortada: se quedo en "Estudio programación" y seguia.
    (Estado::Rota, "Bueno, me llamo Santiago, tengo 21 años. Estudio programación"),
    // Le falta el principio: era "Diseñé un proyecto…".
    (
        Estado::Rota,
        "y se un proyecto de programación el cual hacía una base de datos para inventario \
         llamado inventarios y ayudaba a empresas a optimizar su inventario.",
    ),
    // Diez palabras que no dicen nada, y con el umbral viejo pasaban por una palabra.
    (Estado::Rota, "Ahora mismo. un sistema de bastantes sistema buenos. y bueno"),
    // --- Las dieciocho que sirven -----------------------------------------------------
    (
        Estado::Usable,
        "Una vez tuvo un conflicto con un compañero porque teníamos diferentes pensamientos \
         sobre cómo resolver el problema, pero al final llegamos un acuerdo y entre los dos \
         resolvimos el problema.",
    ),
    (
        Estado::Usable,
        "una vez cometió un error de que no sabía cómo controlar una base de datos de un \
         sistema de una empresa y lo hice a mi manera y al final sin querer borre la base de \
         datos y todos los datos que tenía dentro y fue un error irreversible",
    ),
    (
        Estado::Usable,
        "Trabajo muy bien bajo presión, tengo experiencia trabajando bajo presión, trabajo en \
         una empresa de curtidos, la cual todos los días tengo que trabajar con cierta presión \
         y concentrado.",
    ),
    (
        Estado::Usable,
        "Es decir, desde que empecé a trabajar en la empresa de curtidos, va a día de tenido \
         que dirigir a un equipo de dos personas, en los cuales somos un equipo de tres, pero \
         yo soy el encargado, el cual se encarga de todas las gestiones de la empresa y todos \
         los movimientos dentro de los almacenes.",
    ),
    (
        Estado::Usable,
        "Bueno, porque me gustan mucho los valores de la empresa. La verdad me siento muy \
         identificado con sus valores y sus principios, y la verdad siento que pueda aportar \
         algo bastante interesante en la empresa.",
    ),
    // La mas corta de las buenas: trece palabras, y empieza por "porque" porque le han
    // preguntado "por que". Las dos cosas movieron una constante.
    (
        Estado::Usable,
        "porque la verdad considero que puedo aportar algo bastante interesante en la empresa.",
    ),
    (
        Estado::Usable,
        "Mi punto de fuerte son que soy bueno programando, tengo bastante experiencia \
         trabajando bajo presión y se lleva un equipo bastante estable.",
    ),
    (
        Estado::Usable,
        "La verdad, mi mayor defecto es cuando hay un problema y no sé cómo resolverlo, a \
         veces me puedo cerrar, si no se vien del tema, me informo y ya de ahí podríamos ir \
         tirando.",
    ),
    (
        Estado::Usable,
        "porque quiero ver otras fronteras, la verdad me gusta aprender bastante de varias de \
         varios temas laborales y me gustaría ver otras fronteras sobre trabajos, empresas, \
         etcétera.",
    ),
    (
        Estado::Usable,
        "Con un cliente o compañero enfadado, primero hablaría con él para encontrar una \
         solución, ver cuál es el problema, cuál es el conflicto que tiene referente a ese \
         tema, tratar de llegar a un punto medio entre los dos y ver alguna forma que nos \
         favorezca los dos, lo cual nos dé satisfechos de alguna manera a las dos partes.",
    ),
    (
        Estado::Usable,
        "Cuando no sé cómo resolver algo primero me siento veo cuáles son las herramientas que \
         tengo disponible a mano después de ver cuáles son las herramientas enero prácticamente \
         un mise en plaz en mi mente en el cual ver cómo hacemos las cosas paso a paso dividir \
         un problema en partes y resolviendo cada parte del problema hasta llegar al problema \
         mayor.",
    ),
    (
        Estado::Usable,
        "Estoy orgulloso de mi programa de inventario, el cualice hace poco. para ayudar a \
         empresas ya sean grandes o pequeñas ya que ese es aplicación en específico ayuda solo \
         a empresas muy grandes, pero a pequeñas nunca ayuda.",
    ),
    (
        Estado::Usable,
        "Un día normal en mi trabajo es me levanto las 5 de la mañana, a las 6 de la mañana \
         estoy en el trabajo, a la empezar el día reviso en el sistema, cuáles son si ha \
         llegado alguna devolución para empezar a gestionarla, de ahí veo que tareas puedo \
         ponerle a mis trabajadores.",
    ),
    (
        Estado::Usable,
        "solo trabajar bastante con Cloud Code para proyectos, utilizar Visual Studio Code \
         como Error, como herramienta de programación. utilizó antigravity, utilizó obsidian \
         como tomador de notas. Utilizó también Whisper Flow para la ayuda de transcripciones, \
         etc.",
    ),
    (
        Estado::Usable,
        "La verdad no tengo ninguna pregunta actualmente en el momento que tenga, les avísaré.",
    ),
    (
        Estado::Usable,
        "de momento mi expectativa salarial es dependiendo al acordado por mi trabajo, o sea, \
         si necesitan que haga un trabajo grande por la empresa que ayude bastante a la \
         empresa también espero que así sea mi salario, o sea, mi trabajo va a ser en base a \
         mi salario, es lo que me gustaría.",
    ),
    (
        Estado::Usable,
        "de momento tengo disponibilidad inmediata puedo hacer viajes y mudanzas dependiendo \
         del lo que necesita la empresa y el pago acordado",
    ),
    (
        Estado::Usable,
        "Me llamo Santiago, tengo 21 años, soy programador desde hace un año, he trobarios \
         proyectos, mi fuerte es la integración de APIs y el uso de inteligencia artificial y \
         me muevo bastante bien en distintas áreas y aprendo bastante rápido sobre cualquier \
         cosa que se necesita.",
    ),
];
