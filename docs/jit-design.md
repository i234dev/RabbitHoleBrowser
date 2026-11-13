# JIT para el motor JS de RabbitHole: diseño e implementación

## Estado de las fases

Hay dos numeraciones: **Stage 1** es el estudio histórico y **Stage 2** es
la implementación, subdividida en las fases siguientes. Estar en Stage 2
no significa estar en la fase 2. El resumen del estudio original que sigue
más abajo conserva sus medidas y recomendaciones de entonces.

| Fase | Implementación presente | Cierre pendiente |
|---|---|---|
| 1. Infraestructura | Memoria W^X, ensamblador propio, flags, trazas, estadísticas y validación diferencial | Mantener paridad y comprobar el coste con JIT apagado en cada cambio |
| 2. Código numérico lineal | Constantes, registros, aritmética, comparaciones y retornos | El objetivo original de coste de compilación no está alcanzado |
| 3. Control de flujo | Saltos, latches fusionados, ticks y watchdog | Mantener las pruebas de interrupción y GC-stress |
| 4. Valores dinámicos | Helpers y caminos nativos por tipo; cachés de propiedades/globales; millón diferencial comprobado en `8fd6578` | Mantener paridad y ampliar cobertura del generador; el millón no equivale a conformidad completa |
| 5. Llamadas y excepciones | Llamadas JS/nativas, retornos, constructores, excepciones, cambio de frame sin recursión nativa y operandos estáticos | Medir el binario final tras simplificar llamadas y probar ejecución real en Windows |
| 6. Tiering | Umbrales, exclusiones, fallos cacheados, límite de código por hilo y liberación con la función | Objetivos de carga de páginas, tamaño/coste de compilación y arena de código |
| 7. OSR | Entrada en código nativo durante un bucle interpretado (`LOOP_OSR`) | Mantener paridad al entrar en mitad de la función |
| 8. Rendimiento | Accesos, variables de closure y contadores de referencias en código nativo | Llamadas/retornos y reanudación; demostrar mejora global repetible |
| Extensión S2.8 | GC incremental con barreras para intérprete y JIT | Mantener validación de referencias y pausas bajo cargas reales |

`emit::PHASE = 5` solo controla las **familias de instrucciones** admitidas
por el emisor. Las fases 6–8 y la GC se implementan en otras políticas y
caminos del motor; ese número no describe el avance global.

La funcionalidad de una fase y sus criterios de cierre son cosas distintas.
El JIT sigue apagado por defecto: quedan objetivos medidos sin cumplir.
Los siguientes pasos son cerrar la validación de lo recuperado, reducir el
coste de llamadas/reanudación y después la asignación de memoria ejecutable.
Activarlo requiere paridad funcional y una mejora global demostrada; no
basta con mejorar un bucle numérico aislado.

Estado: **Stage 1 (análisis y diseño) en las secciones 0–10; Stage 2 implementado, ver "Stage 2: implementación" al final**. Rama base del análisis: `main` @ `3896a9e`.
También se ha medido la rama de la "ronda 4 de la VM" (`worktree-agent-ac8f2d59ac314de8f` @ `0d121b8`:
`Value` NaN-boxed de 8 bytes, ICs de cadena de prototipos, caché megamórfica,
asignador por clases de tamaño, latch de bucle fusionado), porque el JIT se
construirá sobre la representación que exista cuando se empiece.

## Validación de la recuperación

- 947 pruebas del workspace en ambos modos, JIT apagado y forzado a umbral 0,
  con resultados idénticos y sin fallos.
- Los últimos arreglos de APIs se comprobaron además con las 80 pruebas de
  `script` en ambos modos y bajo GC incremental forzada. El arreglo de layout
  pasó 125 pruebas de layout, 14 incrementales y el subset CSS de 181 casos.
- `jitdiff`: 40 000 programas aleatorios repartidos en cuatro semillas y
  16 programas del corpus de rendimiento; intérprete y umbrales 0/2 producen
  resultados idénticos. El gate posterior del millón se detalla a continuación.
- `jit-check.sh` rechaza cualquier fallo de Cargo, aunque ambos modos fallen
  igual, y crea la carpeta de resultados cuando se configura explícitamente.
- Las comparaciones de rendimiento deben fijar afinidad en máquinas con
  núcleos de distinta velocidad: medir CPU del hilo no elimina esa variación.
  No se ha activado el JIT por defecto ni se ha añadido un backend ARM64.

## Operandos estáticos y gate del millón

La revisión `8fd657859336b22379335078522a0308be159b25` incorpora los operandos
de `CallR` y el registro fuente de `ReturnR` como inmediatos del código generado.
Los helpers evitan releer y decodificar esos operandos de `code.ops`; conservan
`call_r`/`return_r`, los frames, la captura de pánicos, las excepciones, los
puntos de GC y el watchdog. No añade campos al estado ni cambia la reanudación.

Sobre esa revisión, en Linux x86-64:

- **1 000 000 de programas aleatorios distintos, cero diferencias** entre el
  intérprete y JIT con umbrales 0 y 2. Se ejecutaron ocho particiones de 125 000
  programas con semillas iniciales `1`, `125001`, `250001`, `375001`, `500001`,
  `625001`, `750001` y `875001`: cubren exactamente las semillas 1–1 000 000.
  Los ocho procesos terminaron con código 0; sus contadores finales y el
  manifiesto suman el millón. Los 100 115 programas con excepción no capturada
  dieron el mismo resultado en los tres modos; no son diferencias del JIT.
- El binario congelado de `jitdiff` tiene SHA-256
  `778381450bacb422d8de0c9dd8506269e21eeee5a3bbd57d3c39f07d86f08026`.
  Se conservaron el manifiesto, los ocho logs y los rangos de semillas. No se
  atribuye esta ejecución a cambios posteriores a `8fd6578`.
- El paquete JS pasa **193 pruebas con JIT apagado y 193 encendido**, con
  resultados idénticos; incluye 172 unitarias, GC de memoria, 18 del parser
  y dos del harness test262. No se repitió el corpus completo de test262.
  Las 14 pruebas JIT también pasan con umbral 0 y `RH_GC_STRESS=inc`.
  Una regresión nueva cubre 300 parámetros, 300 sitios de llamada y retornos
  de strings desde registros que exceden el rango de un byte.

El millón compara contra el intérprete propio; no demuestra por sí solo
conformidad de JavaScript ni cobertura exhaustiva de los programas posibles.

Antes de limpiar la implementación se compararon ocho rondas alternas del
**mismo binario experimental**, fijado a CPU 0 y sin otras compilaciones.
Cada ronda ejecutó los 27 casos de `perf.rs`, incluidos parseo y compilación
JIT. Los operandos estáticos redujeron el tiempo de `calls` un 4,54 %, de
`closure_calls` un 5,18 % y de `method_calls` un 3,13 %. El total pasó de
390,285 a 389,410 ms de CPU del hilo (−0,22 %: paridad práctica);
`push_objects` y `array_map` empeoraron un 1,97 % y un 1,75 % respectivamente.

El RSS máximo fue de unos 38 MiB en ambos modos. En una pasada separada de
estadísticas, agregando 81 realms, se compilaron las mismas 111 funciones y
2367 instrucciones: 370 512 → 370 836 bytes de código (+0,087 %) y
3,44 → 3,81 ms de compilación. Esa pasada única y redondeada no permite
concluir un cambio robusto del coste de compilación. `MAP_NS` es acumulativo
por hilo y no debe sumarse como si fuera un contador por realm.

Estas cifras corresponden al experimento, antes de quitar el interruptor y
los helpers duplicados. En él, `push_objects` con JIT alcanzó solo 0,9123×
el throughput del intérprete, por debajo del gate de 0,95×.

### Medición del binario final

El 14 de noviembre de 2026 se compararon los binarios congelados anteriores
al cambio y de `8fd6578`, sobre Intel Core i7-12700H, Linux x86-64, afinidad
CPU 0 y sin compilaciones, fuzz ni otros tests activos. Se alternaron ocho
rondas de cuatro modos: anterior apagado/encendido y final apagado/encendido.
Son los 27 casos de `perf.rs`; cada dato es la mediana de ocho mínimos de tres
evaluaciones en realms nuevos, con parseo y compilación JIT incluidos. El JIT
usó su umbral predeterminado de 1000. Se midió CPU del hilo; las anomalías del
reloj de pared de WSL hacen que los tiempos de `/usr/bin/time` no sean una
fuente válida de duración para esta comparación.

| Modo | Anterior | Final | Cambio de tiempo |
|---|---:|---:|---:|
| JIT apagado, suma de los 27 casos | 585,285 ms | 574,160 ms | −1,90 % |
| JIT encendido, suma de los 27 casos | 387,310 ms | 382,945 ms | −1,13 % |

Con JIT encendido, `calls` pasó de 9,035 a 8,255 ms (−8,63 %),
`closure_calls` de 11,015 a 10,380 ms (−5,76 %) y `method_calls` de
11,335 a 10,830 ms (−4,46 %). Sus controles apagados también cambiaron
−2,40 %, −5,89 % y −5,46 % respectivamente. La mejora mayor del total
apagado impide atribuir una mejora global específica al JIT. La comparación
experimental dentro del mismo binario y esta comparación entre binarios
finales son evidencias distintas y no deben confundirse.

El RSS máximo mediano con JIT fue 38 296 → 38 396 KiB. En una pasada separada
de estadísticas, las mismas 111 funciones y 2367 instrucciones ocuparon
370 512 → 370 836 bytes; la suma de compilación fue 2,86 → 2,90 ms.
El tiempo acumulado final de mapeo del hilo fue 1,38 → 1,39 ms, sin sumar
sus valores intermedios. Esa única pasada no demuestra una mejora robusta
del coste de compilación. Los logs registran los 32 procesos, estadísticas
separadas y estos SHA-256:

- Anterior: `6b7c9af467db7cd76ee96ac1fb1d0039fce7d01ed16c501a1f68890bc8edfe9d`.
- Final: `4afc429579a566f54c4ea1b27ccf193c3276d8111d1069829df6ba383cfa7d51`.

En esta tanda final el mínimo de throughput JIT/intérprete fue 0,9571×
(`push_objects`), frente al 0,9123× observado en el experimento. No se ha
repetido Speedometer para este cambio ni se ha demostrado una mejora global
específica del JIT. Los gates de rendimiento, páginas, coste de compilación
y memoria ejecutable siguen abiertos. **El JIT continúa apagado por defecto.**

## Resumen histórico del estudio

1. **Hoy no es la mejor inversión. Recomendación: "todavía no"** para el JIT,
   pero sí hacer ya la fase 1 de infraestructura de medida (contadores) como
   puerta de decisión. Datos:
   - En páginas reales el intérprete propiamente dicho (bucle de dispatch +
     fast paths + `exec_op` + maquinaria de llamadas + ICs) es el **8–9 % del
     tiempo de JS** en Wikipedia y en un fixture DOM, y el **21 %** en YouTube
     (≈ 11 % de la CPU total del proceso). El resto es DOM/bindings, malloc,
     búsqueda de propiedades, strings, parseo/compilación y layout.
   - Un prototipo desechable de JIT base con las restricciones de Level 1
     (valores siempre en el frame en memoria, sin especialización ni OSR) da
     **1,2–1,3×** en un bucle de una sola instrucción, **1,5×** en `s += i % 7`
     y **2,7×** en un bucle de 7 instrucciones numéricas, frente al intérprete
     de la ronda 4.
   - Pero sin OSR, los bucles de código que se ejecuta una vez (cuerpo del
     script, IIFE) **nunca** se compilan: eso es casi todo `perf.rs`. Y las
     llamadas JS→JS siguen pasando por los frames de la VM, que es donde más
     perdemos frente a QuickJS (fib 28 ms frente a 11 ms; `calls` 33 frente a 16).
   - Estimación de ganancia de Level 1: ≤ 2–4 % de CPU en YouTube, ≤ 4 % en el
     fixture DOM, ≈ 0 % en la mayoría de `perf.rs`. Hay alternativas medidas más
     baratas y con más retorno (sección 2.8): coste de las llamadas, globales,
     hashing/wrappers del DOM, malloc, parseo.
2. **Backend: generador propio** (emisor de plantillas x86-64, después aarch64),
   no Cranelift. Con los mismos ops y la misma estrategia, el prototipo propio
   compila en **0,06–0,12 µs por instrucción** frente a **34–52 µs** de Cranelift
   (≈ 500× más), genera código igual de rápido en Level 1 (89 ms frente a 96 ms
   en el bucle de 7 ops) y de tamaño parecido (61–80 B/op frente a 62–78 B/op),
   sin añadir dependencias (Cranelift: +38 crates, +4,9 MB de binario,
   +212 s de CPU en build limpia). Cranelift queda como opción para un futuro
   nivel optimizador.
3. El diseño de Level 1 (secciones 4–7) está listo para cuando se abra la puerta:
   función entera o nada, valores siempre en el frame, helpers que devuelven un
   código de estado, llamadas JS→JS que vuelven a la VM y se reanudan en
   puntos de continuación (sin recursión nativa), excepciones resueltas por la
   VM, W^X, código liberado con su `FunctionCode`.

---

## 0. Método y entorno de medida

- Máquina: Intel Core i5-7500T (4 hilos lógicos visibles como 3 CPUs, 2,7 GHz),
  carga media 5–8 por otros agentes. Todos los tiempos son **tiempo de CPU del
  hilo** (`CLOCK_THREAD_CPUTIME_ID`), mejor de 3 ejecuciones, y se repitieron
  al menos dos veces en la misma sesión.
- Perfiles: muestreador propio `RH_PROF` (SIGPROF + cadena de frame pointers;
  binarios con `CARGO_PROFILE_RELEASE_STRIP=false` y
  `RUSTFLAGS=-Cforce-frame-pointers=yes`). La resolución efectiva del timer
  es ~1 ms, así que se acumularon varias ejecuciones (de 300 a 5000 muestras
  por caso). La simbolización se hizo con un script Python que lee `.symtab`.
- Para atribuir tiempo **por opcode** se usó una copia desechable de la ronda 4
  con una línea extra en el bucle de dispatch (guarda la etiqueta del op en
  curso en una atómica que el manejador de la señal copia en la muestra) y un
  contador dinámico de ops (`RH_OPCOUNT`). Esa instrumentación no se ha
  comiteado (tocaba `vm.rs`/`sampler.rs`); el método está en el apéndice A.
- Los símbolos `libc:*` salen de la tabla dinámica de libc (sin símbolos
  internos), así que agrupan malloc/free/memcpy internos; se reportan juntos.
- Las categorías "intérprete" agrupan: `run_inner` (dispatch + fast paths
  inline), `exec_op` y conversiones lentas, maquinaria de llamadas/frames e ICs.

---

## 1. Arquitectura actual (`crates/js`)

### 1.1 Pipeline

`lexer.rs` → `prescan.rs` (parseo perezoso: los cuerpos de funciones se saltan
y se guardan como `LazyCode`) → `parser.rs`/`ast.rs` → `captures.rs` (qué
nombres captura alguna función anidada) → `compiler.rs` genera bytecode **de
pila** → `regs.rs::translate` lo reescribe a **forma de registros** →
`FunctionCode` (`bytecode.rs`). Las funciones se compilan la primera vez que se
llaman (`ensure_compiled` → `compile_lazy`; el resultado se comparte en
`FunctionCode::compiled: OnceCell`).

### 1.2 Formato de bytecode y bucle de dispatch

- `Op` es un `enum` Rust `Copy` con operandos inline (212 variantes en `main`,
  214 en la ronda 4, que añade `IncJumpIfLtRR/RK`). Una función es un
  `Vec<Op>` más tablas: `constants`, `functions`, `handlers`, `ics`
  (`Box<[Cell<PropIc>]>`), `ic_keys`, `depths` (profundidad de pila estática por
  instrucción), `positions`.
- Formas de registro (producidas por `regs.rs`): `Mov`, `LoadK`, `AddRR/RK`…
  `ModRR/RK`, `BinRR/RK`, `CmpRR/RK`, saltos fusionados comparar-y-saltar
  (`JumpUnlessLtRK`, `JumpIfStrictEqRR`…), `JumpIfTrueR`…, `IncR/DecR/NotR/UnR`,
  `GetPropR/SetPropR/DefinePropR`, `GetElemR/SetElemR`, `GetGlobalR/SetGlobalR`,
  `GetLocalR/SetLocalR`, `CallR/NewR/ReturnR`, `LoadThis`, `StoreCompletionR`.
  Los operandos son **índices absolutos de slot del frame**: primero los
  `nregs` registros y después la pila de operandos (el valor a profundidad `d`
  vive en el slot `nregs + d`). Lo que no tiene forma de registro sigue siendo
  una instrucción de pila cuyos operandos están en profundidades conocidas
  estáticamente (`depths`). Si el frame no cabe en `u16`, se deja el código de
  pila sin traducir (`untranslated`).
- `vm.rs::run_inner` es un `loop { match op }` (dispatch por `match`, Rust
  estable no tiene computed goto). Mantiene `pc` y un puntero base `base` a los
  slots del frame en locales; solo los **fast paths** viven ahí (números,
  concatenación de strings, aciertos de IC, llamadas rápidas). Cualquier fallo
  o instrucción de pila sale a `exec_op(fi, op)` (`#[inline(never)]`), que
  implementa **la semántica completa de cada instrucción** (el macro `slow!`
  sincroniza `pc`, llama, y recarga `pc`/`base` porque `vstack` puede haberse
  movido).
- `tick`: los saltos hacia atrás y las llamadas decrementan `realm.tick`; cada
  1024 se llama a `safepoint()` (GC, watchdog, muestreo `RH_JSPROF`).

### 1.3 Registros, frames, variables, closures y scopes

- `Realm::vstack: Vec<Value>` contiene los slots de todos los frames activos;
  `Realm::frames: Vec<Frame>`. `Frame` = `code: Rc<FunctionCode>`, `pc`,
  `base`, `argc` (argumentos conservados debajo de `base` en modo
  `args_direct`), `scope: ObjectId`, `scope_depth`, `this`, `func`,
  `new_target`, `home_object`, `ret_slot` (slot del llamador que recibe el
  resultado), `is_construct`, `entry` (volver de este frame sale del bucle) y
  `extra: Option<Box<FrameExtra>>` (objeto `arguments`, generador, valor de
  compleción, slots guardados de un frame suspendido).
- **`vstack` es un `Vec` que crece**: cualquier operación que reserve slots
  (empujar un frame, una llamada nativa que reentra) puede **reubicarlo**. Por
  eso el intérprete recarga `base` tras cada salida de línea. El JIT tendrá la
  misma obligación.
- Variables: las que ninguna función anidada menciona (análisis por nombre,
  sobreaproximado) van a **registros del frame**; las capturadas van a un
  **objeto Scope del heap** (`ObjectKind::Scope(ScopeData { slots, parent })`)
  direccionado por `(profundidad, slot)` (`GetLocalR/SetLocalR`). Los bloques
  con `let` capturados hacen `PushScope/PopScope/CopyScope` (este último para
  bindings por iteración). El TDZ se representa con `Value::Empty`
  (`MovChecked`, `CheckInit`, `GetLocalChecked`).
- Closures: `ObjectKind::Function(Closure { code, scope, this_value,
  new_target, home_object, fields, arguments })`; el `scope` capturado es el
  padre del scope de la llamada.
- Globales: `GetGlobalR/SetGlobalR` con IC (`IC_GLOBAL_LEX` para `let/const`
  de script indexados en `GlobalLex`, `IC_GLOBAL_OBJ` para propiedades del
  objeto global), validados por una época léxica.

### 1.4 Llamadas y reentrada

- **JS→JS sin recursión nativa**: `CallR` → `try_fast_call` (closure "plana":
  sin `arguments` materializado, sin rest, no generador/async/constructor de
  clase) mueve los argumentos del llamador directamente a un frame nuevo y hace
  `continue 'reload`; `ReturnR` hace `pop_frame`, escribe el resultado en
  `ret_slot` del llamador y `continue 'reload`. El caso general pasa por
  `exec_op` → `do_call` → `push_plain_call`/`prepare_frame` + `push_frame`.
  `NewR` → `try_fast_construct`.
- **JS→nativo**: `try_native_call` llama a `NativeFn = fn(&mut Realm, Value,
  &[Value]) -> JsResult<Value>` vía `call_native` (incrementa `native_depth`,
  `current_callee`, `new_target_stack`); algunos intrínsecos (`charCodeAt`,
  `fromCharCode`, `Math.imul`) se calculan inline; los getters nativos cacheados
  por IC (DOM) se llaman directamente.
- **Nativo→JS (reentrada)**: `Realm::call`/`construct` empujan un frame con
  `entry = true` y ejecutan un `run()` **anidado** en la pila nativa. La
  protección de pila (`stack.rs`) compara el puntero de pila con los límites
  reales del hilo y lanza `RangeError` antes de desbordar; `max_frames = 12 500`
  limita la profundidad JS.
- En la ronda 4: `f.call`/`f.apply`/funciones ligadas empujan el frame destino
  directamente (sin `run()` anidado) y hay un hook `post_native` del host.

### 1.5 Inline caches (`ic.rs`)

- Cada `GetPropR/SetPropR/DefinePropR/GetGlobalR/SetGlobalR` tiene un
  `PropIc { kind, shape, slot, holder, holder_shape, mid, mid_shape }` en
  `FunctionCode::ics` (celdas `Cell`). Clases en `main`: `IC_OWN` (forma +
  slot), `IC_OWN_DICT`, `IC_PROTO` (receptor de forma X, propiedad en `holder`
  a través de `mid`), `IC_PRIM`, `IC_LENGTH` (arrays y strings), `IC_ADD`
  (transición de forma al añadir), `IC_GLOBAL_LEX`, `IC_GLOBAL_OBJ`.
- Monomórficas; el camino lento rellena el registro (`ic_fill_get`,
  `ic_fill_set`) y el rápido revalida con comparaciones de id de forma
  (`ic_get`, `ic_set`). Un acierto en un accessor nativo se llama directamente.
- La ronda 4 añade IC de cadena de prototipos profunda validada por una "época
  de prototipos", setters y getters JS cacheados, `IC_ABSENT`, una caché
  megamórfica compartida `(forma, nombre)` y hashes de string cacheados.

### 1.6 Excepciones y try/catch/finally

- Todo es `JsResult<T> = Result<T, Value>` propagado con `?`. **No hay
  unwinding nativo para excepciones JS**.
- `run()` envuelve `run_inner()`; ante `Err(e)` llama a `unwind(e)`: busca en
  `code.handlers` (`[start, end) → target`, con `stack_depth` y `scope_depth`)
  usando `frame.pc - 1`, restaura la cadena de scopes, deja la excepción en el
  slot `nregs + stack_depth` y salta al `target`; si no hay manejador, saca el
  frame y sigue con el llamador; al llegar a un frame `entry` devuelve `Err` al
  código Rust que hizo la llamada. Un timeout del watchdog (`terminating`) no
  es capturable.
- `finally` se compila **inline** en cada salida (return/break/continue/caída)
  más un manejador catch-all que relanza. No hay instrucciones especiales de
  finally.
- Los pánicos de Rust (p. ej. `Heap::get` sobre un objeto liberado) se
  propagan como unwinding normal de Rust.

### 1.7 Generadores y async/await

- Sí se suspenden frames: `Yield/Await` (en `exec_op`) guardan el slot de
  reanudación, **sacan el frame** de `frames`, mueven sus slots de `vstack` a
  `FrameExtra::saved` (`detach_slots`) y devuelven `Exit::Yield/Await` con el
  `Frame` en una caja, que vive en el objeto generador. Reanudar
  (`generator_resume_raw`) vuelve a colocar los slots al final de `vstack`
  (**con otro `base`**) y llama a `run()`; `throw` se inyecta con `unwind`.
- async = generador con promesa; `await_value` encadena `then` nativos que
  reanudan el frame. `return()` sobre un generador suspendido no ejecuta
  `finally` (limitación conocida, ya documentada en el código).

### 1.8 Representación de `Value`

| | `main` | ronda 4 |
|---|---|---|
| Tamaño | 16 bytes | 8 bytes |
| Representación | `enum` con `#[repr(u64)]`: palabra de etiqueta (Undefined=0, Null=1, Bool=2, Number=3, String=4, Symbol=5, BigInt=6, Object=7, Empty=8) + palabra de payload | NaN-boxing: un número es su patrón IEEE (< `TAG_MIN = 0xFFF9_0000_0000_0000`); el resto vive en el espacio de NaN negativos: `0xFFF9` especiales (undefined=…0, null=…1, empty=…2, false=…4, true=…5), `0xFFFA` símbolo, `0xFFFB` objeto (id u32), `0xFFFC` BigInt (`Rc`), `0xFFFD` string (`Rc`) |
| ¿Layout estable? | Sí: `#[repr(u64)]` en un enum con datos tiene layout definido (RFC 2195): etiqueta u64 en +0, payload en +8 | Sí: `#[repr(transparent)] struct Value(u64)` |
| Con contador de referencias | String, BigInt (`Rc`) | `bits >= TAG_BIGINT` (`0xFFFC…`) |
| Comprobación "es número" | `qword [slot] == 3` | `bits < TAG_MIN` (una comparación sin signo) |
| NaN | — | canonicalización: `Value::Number`/`set_number` convierten cualquier patrón `>= TAG_MIN` en `0x7FF8…` |

Consecuencias para código nativo: ambos layouts se pueden leer y escribir
directamente; los objetos son **índices** (`ObjectId(u32)`) en una arena, no
punteros; copiar o sobrescribir un valor con `Rc` exige ajustar contadores
(clone/drop), así que el código generado **no debe copiar ni sobrescribir
valores con contador**: eso va a un helper.

### 1.9 GC (`heap.rs`, `realm.rs`)

- Mark-and-sweep en una arena `Vec<Option<Object>>`; los ids se reciclan con
  una lista libre. **No mueve objetos** ni es generacional. *(Estado de
  stage 1; desde S2.8 es incremental, con barrera de escritura en
  `Heap::get_mut`.)*
- Raíces (`Realm::gc`): intrínsecos y global, `global_lex`, cada `Frame`
  (`scope`, `this`, `func`, `new_target`, `home_object`, extras y slots
  guardados), **todo `vstack`**, jobs, la tabla de `Root` (handles), `extra_roots`
  del host (wrappers DOM), `job_roots`, `new_target_stack`, promesas no
  manejadas, `template_cache`, módulos y `current_callee`. Las strings son
  `Rc` (fuera del GC).
- **La GC solo corre en `safepoint()` de la VM y solo si `native_depth == 0`**
  (ninguna función nativa activa; así no hace falta rastrear locales Rust).
  Pero un helper que ejecute JS (getter, `valueOf`, proxy, llamada) entra en
  un `run()` anidado que sí puede alcanzar un safepoint: **cualquier llamada que
  pueda ejecutar JS es un posible punto de GC y de reubicación de `vstack`**.

### 1.10 Lo que esto implica para un JIT

| Hecho | Implicación |
|---|---|
| Frames y slots ya viven en memoria (`vstack`), con layout fijo y profundidades estáticas | El JIT puede usar **exactamente el mismo frame** que el intérprete: entrar y salir en cualquier frontera de instrucción es trivial (no hay estado que reconstruir) |
| `exec_op` implementa cada instrucción completa | Helper genérico: el JIT no reimplementa semántica, llama a `exec_op` para todo lo que no sea fast path |
| JS→JS sin recursión nativa | El JIT debe preservarlo: las llamadas vuelven a la VM y la función se reanuda en un punto de continuación |
| `vstack` se reubica | Recargar la base de slots tras **cada** helper |
| GC no móvil, sin barreras, raíces = todo `vstack` | Ningún mapa de pila ni barrera si los valores viven en el frame |
| Excepciones = `Result`, desenrollado por la VM | Los helpers devuelven un estado; la VM resuelve `catch` con la tabla de handlers existente |
| `Rc` en strings/BigInt | Fast paths solo con valores sin contador; el resto, helper |

---

## 2. Perfil medido

### 2.1 Micro-benchmarks (`crates/js/examples/perf.rs`, ms de CPU)

| caso | `main` | ronda 4 | QuickJS (zig cc -O2) |
|---|---:|---:|---:|
| empty_loop | 71,8 | 50,6 | 112,9 |
| global_loop | 78,3 | 77,8 | 30,1 |
| local_loop | 22,3 | 20,0 | 21,9 |
| let_loop | 28,2 | 27,6 | 29,1 |
| push_objects | 51,6 | 41,2 | 45,5 |
| calls | 37,5 | 33,2 | 16,4 |
| closure_calls | 47,6 | 47,6 | 17,9 |
| method_calls | 47,3 | 48,9 | 22,2 |
| new_objects | 97,3 | 82,8 | 48,9 |
| props | 37,9 | 42,6 | 27,7 |
| prop_set | 55,2 | 57,7 | 34,9 |
| array_rw | 50,3 | 52,0 | 22,2 |
| strings | 9,9 | 5,9 | 11,7 |
| string_ops | 41,6 | 46,8 | 25,1 |
| array_map | 83,3 | 65,7 | 53,9 |
| fib | 28,9 | 28,0 | 10,9 |
| json | 80,4 | 59,1 | 68,4 |
| for_in | 46,8 | 41,8 | 41,0 |
| try_catch | 9,8 | 10,6 | 4,8 |
| arguments | 40,7 | 39,2 | 34,1 |
| switch | 43,1 | 47,3 | 22,5 |
| **TOTAL** | **1010** | **926** | **702** |

Lectura: nuestro intérprete ya es **más rápido que QuickJS en bucles numéricos
monomórficos** (empty_loop 2,2×). Donde perdemos de 2 a 2,7× es en
**llamadas** (fib, calls, closure_calls, method_calls), **globales**
(global_loop) y accesos a arrays/propiedades. Ninguna de esas pérdidas es de
dispatch.

### 2.2 Reparto del tiempo en `perf.rs` (ronda 4, 3354 muestras)

Por categoría (tiempo propio): núcleo del intérprete (`run_inner`: dispatch +
fast paths inline) **68,1 %**, llamadas/frames 6,2 %, propiedades/objetos 5,9 %,
drop/clone de `Value` 4,3 %, strings 2,9 %, ICs 2,2 %, `exec_op` 1,8 %, otros
(arena, asignador) 7,7 %. En micro-benchmarks el dispatch domina por diseño.

**Top 10 opcodes por tiempo** (inclusivo: incluye los helpers que llaman):

| op | % tiempo | dónde se va |
|---|---:|---|
| GetGlobalR | 20,8 | 98 % en `run_inner` (fast path de IC global) |
| CallR | 19,9 | frames 29 %, `run_inner` 24 %, nativos 21 % |
| SetGlobalR | 7,8 | `run_inner` |
| IncJumpIfLtRK | 6,4 | `run_inner` |
| AddRR | 5,5 | `run_inner` 81 %, concatenación 13 % |
| ReturnR | 4,7 | `run_inner` 88 %, drop 11 % |
| ModRK | 4,1 | `run_inner` (división entera) |
| GetPropR | 4,0 | `run_inner` 63 %, `ic_get` 34 % |
| NewR | 2,8 | asignación 31 %, `run_inner` 29 %, formas 21 % |
| JumpIfLtRK | 2,5 | `run_inner` 61 % |

**Top opcodes por frecuencia** (273,7 M ops ejecutados): GetGlobalR 25,6 %,
IncJumpIfLtRK 14,1 %, SetGlobalR 10,7 %, AddRR 7,9 %, StoreCompletionR 7,2 %,
IncR 4,8 %, JumpIfLtRK 4,4 %, GetPropR 3,8 %, ModRK 2,5 %, LoadK 2,5 %.
Globales leídas/escritas = **36 % de todos los ops** (código de nivel superior).

**Top helpers** (tiempo propio, toda la suite): `Vec<Frame>::push` 2,2 %,
`Heap::alloc` 1,8 %, `push_plain_call` 1,5 %, `ic_get` 1,5 %,
`try_native_call` 0,9 %, `Vec<Value>::extend_with` (inicializar slots de
frame) 0,9 %, `PropertyMap::push_with_shape` 0,8 %, `exec_op` 0,8 %,
`ic_set` 0,75 %, drop de `Vec<Value>` 0,7 %.

Casos sueltos:
- `empty_loop`: 97,9 % en `IncJumpIfLtRK`. `local_loop`: `ModRK` 61 %, `AddRR`
  19 %, latch 15 %.
- `fib`: `CallR` 40 % + `ReturnR` 23 % + `GetGlobalR` 10 %; 11,6 % propio en
  maquinaria de frames. `calls`: `CallR` 30 %, `ReturnR` 15 %, globales 25 %.
- `new_objects`: solo 48 % en `run_inner`; `NewR` 26 %, `SetPropR` 20 %
  (transiciones de forma).
- `array_map`: `CallR` 71 % (map/filter nativos que llaman al callback: 36 %
  propio en llamadas/frames). `json`: 7 % en `run_inner`, el resto builtins.

### 2.3 Benchmark de manipulación DOM (fixture local)

Página local (`file://`) con funciones estilo framework: crear una tabla de
1500×6 con `createElement`/`textContent`/`setAttribute`, recorrer
`querySelectorAll` cambiando `classList`/`style`/atributos, `innerHTML` de
1500 `<li>` y lectura de `data-*`, 20 000 `dispatchEvent(CustomEvent)`, y un
mini-virtual-DOM (300 nodos, 60 rondas de render + diff + patch). Ejecutado con
`crates/engine/examples/shot.rs` (`main`, 4 ejecuciones, 3852 muestras).
Tiempos: tabla 48 ms, touch 45 ms, innerHTML 43 ms, eventos 103–113 ms,
vdom 479–517 ms.

El 81 % de la CPU del proceso está bajo ejecución JS. **Dentro del JS**:

| categoría | % del tiempo JS |
|---|---:|
| Bindings DOM (`crates/script`, `crates/dom`) | 43,0 |
| libc (malloc/free/memcpy) | 25,8 |
| propiedades/objetos | 5,5 |
| núcleo del intérprete (`run_inner`) | 4,6 |
| strings | 3,8 |
| drop/clone | 2,4 |
| `exec_op` + llamadas + ICs | 3,6 |

Puntos calientes: `script::host::wrap` 13,6 %, **SipHash** (`HashMap` con
`RandomState` en el host de script) 13,1 %, materializar `Vec<NodeRef>` para
`childNodes` 5,2 % + `n_child_nodes` 3,2 %. **El intérprete propiamente dicho
es el 8,2 % del tiempo de JS.**

### 2.4 Páginas reales (`shot`, `main`)

- **Wikipedia** (artículo "JavaScript", 1274 muestras): JS = 38 % de la CPU;
  dentro del JS el intérprete propiamente dicho es el **8,8 %** (3,4 % del
  total). Domina malloc/libc (39 % del total) y estilo/layout (31 %).
- **YouTube** (portada, 6 s de espera, 5153 muestras): JS = 55 % de la CPU.
  Dentro del JS: `run_inner` 13,9 %, `exec_op` 4,0 %, llamadas 2,6 %, ICs 0,4 %
  → **20,9 %** (≈ 11,5 % del total). libc 26,6 %, propiedades/objetos 13 %
  (`Shape::find` 6,2 %), strings 4,7 %, GC 0,9 %. El parseo y la compilación JS
  son el **11,7 % de la CPU total**.

### 2.5 Ganancia medida de un JIT base (prototipo desechable)

Prototipo fuera del repo (borrado al terminar): un bytecode de registros
mínimo (`LoadK`, `Mov`, `AddRR/RK`, `ModRK`, `IncR`, `JumpUnlessLtRK`, `Jump`,
`ReturnR`, "genérico"), sobre `Value` NaN-boxed de la ronda 4, **con las reglas
de Level 1**: todo valor se lee y escribe en el frame en memoria, comprobación
de tipo en cada operando, almacenamiento solo si el valor anterior no tiene
contador, caminos lentos a helpers `extern "C"` que devuelven un estado, tick
de safepoint en saltos hacia atrás, base del frame recargada tras cada helper.
Dos generadores: Cranelift 0.137 y un emisor de plantillas x86-64 propio
(~250 líneas). Comparado con el intérprete en la misma sesión (ms de CPU):

| bucle | `main` | ronda 4 | JIT L1 Cranelift | JIT L1 propio | Cranelift con valores en registros (ya no L1) |
|---|---:|---:|---:|---:|---:|
| `for (i<1e7){}` (1 op/iter) | 70,7 | 50,3–53,7 | 43,2–43,6 | 40,6–41,9 | 34 |
| `s += i % 7` (1e6) | 21,7–23,6 | 19,8–20,1 | 13,6–14,3 | 12,5–13,4 | 13,3 |
| 7 ops numéricas (1e7) | 255 | 243 | 96–97 | 89 | 90–93 |

Speedup de Level 1 sobre la ronda 4: **1,2–1,3×** con una instrucción por
iteración (el camino crítico es la dependencia a través de memoria
store→load + `addsd`, que el intérprete ya oculta con un salto bien predicho),
**1,5×** con `%`, **2,7×** con 7 ops por iteración (ahí sí pesa el dispatch).
Tener los valores en registros (lo que haría un nivel 2) apenas añade sobre
L1 en estos bucles.

### 2.6 Por qué esa ganancia no llega a las cargas reales

1. **Sin OSR no se compilan los bucles de código que se ejecuta una vez.** En
   `perf.rs`, 19 de 21 casos tienen su bucle en el cuerpo del script o en una
   IIFE (los otros dos, `fib` y `array_map`, no tienen bucle JS): con umbral
   > 0 esos bucles no ganan nada. Solo se compilarían los callees
   (`f(x){return x+1}`, constructores, callbacks de `map`), que son
   diminutos.
2. **Las llamadas siguen en la VM.** fib/calls/method_calls pasan del 45 al
   63 % del tiempo en `CallR`/`ReturnR` (crear y destruir frames); el JIT de
   Level 1 no lo cambia.
3. **En páginas reales el intérprete es una fracción pequeña del JS**
   (8–21 %), y solo la parte de dispatch dentro de funciones calientes es
   recuperable.

Estimación de Level 1 (cota superior): YouTube ≤ 2–4 % de CPU (la mitad del
`run_inner` que cae en funciones calientes); fixture DOM ≤ 4 %; Wikipedia
≤ 2 %; `perf.rs` total < 5 %; funciones calientes con bucles numéricos de
varias instrucciones 2–2,7×. Coste: ~3–5 k líneas más tests, y mantener para
siempre un segundo motor de ejecución que debe coincidir con el intérprete.

### 2.7 Alternativas más baratas (medidas)

| Alternativa | Dato que la respalda | Ganancia estimada | Coste |
|---|---|---|---|
| A. Bindings DOM: hash rápido (Fx/identidad) en los mapas del host, caché de wrappers en el nodo, `childNodes` vivo sin materializar `Vec` | SipHash 13,1 % + `wrap` 13,6 % + `childNodes` 8,4 % del JS en el fixture DOM | 20–30 % del tiempo JS en código DOM | días; fuera de `crates/js` |
| B. Llamadas más baratas: `Frame` más pequeño, sin `Rc::clone` del código por llamada, no asignar scope ni rellenar slots innecesarios, ICs de llamada | `CallR`+`ReturnR` = 45–63 % en fib/calls; QuickJS 2,6× más rápido ahí | 1,3–2× en código con muchas llamadas, **también en código que corre una vez** | 1–2 semanas; la ronda 4 ya trabaja en ICs de llamada |
| C. Globales más baratas (slots directos) | 36 % de los ops de la suite; QuickJS 2,6× en `global_loop` | 1,5–2× en código de nivel superior | en curso en la ronda 4 |
| D. Propiedades: caché megamórfica, menos `Shape::find` | 13 % del JS de YouTube | 5–10 % del JS en frameworks | hecho en parte en la ronda 4 |
| E. Parseo/compilación: más pereza, caché de bytecode por URL | 11,7 % de la CPU total en YouTube | 5–10 % de la carga | 1–2 semanas |
| F. Asignador por clases de tamaño | libc 26–27 % del JS (DOM, YouTube) | ya medido por la ronda 4 (malloc/free = 1/5 de YouTube) | hecho en la ronda 4 |
| G. Más superinstrucciones (método+llamada, carga+guarda, latches) | latch fusionado: empty_loop 71→51 ms | 10–30 % en bucles | días cada una |
| H. Representación de `Value` (NaN-boxing) | suite 1010→926 ms; strings 9,9→5,9; json 80→59 | ya hecho en la ronda 4 | — |

### 2.8 Conclusión de 1.3

**Todavía no.** Orden recomendado: A y B (ganancia grande, también en código
frío), dejar terminar C/D/F/G a la ronda 4, luego E. **Puerta para reabrir el
JIT**, medida con la fase 1 del plan (contadores, sin generar código):

1. el intérprete propiamente dicho (dispatch + fast paths + `exec_op`) supera
   el 25–30 % del tiempo JS en el conjunto de páginas de referencia tras A–C, y
2. al menos el 50 % de los ops ejecutados caen en funciones que superan el
   umbral de invocaciones (lo que Level 1 puede acelerar sin OSR).

Si la puerta se abre antes, el diseño siguiente es el que se debe implementar.

---

## 3. Backend: Cranelift o generador propio

Prototipo: los mismos ops y la misma estrategia L1 con ambos generadores
(sección 2.5); funciones sintéticas de 10 a 5000 ops (60 % llamadas a helper,
40 % aritmética/saltos con fast path). Tiempo de compilación = CPU del hilo,
media de 3–20 repeticiones; Cranelift con `opt_level=speed`, verificador
desactivado, unwind info activado.

| Criterio | Cranelift 0.137 (`cranelift-codegen` + `frontend`) | Generador propio (plantillas) |
|---|---|---|
| Compilación por instrucción | **34–52 µs** (10–1000 ops); 141 µs con 5000 ops (crecimiento superlineal); `opt_level=none`: 23–37 µs | **0,06–0,12 µs**, lineal |
| Función típica de 200 ops | 6,7 ms | 13,5 µs |
| 500 funciones calientes de 50–200 ops (página pesada) | ≈ 1–3,5 s en el hilo principal | ≈ 2–7 ms |
| Velocidad del código L1 | bucle de 7 ops 96 ms; `%` 13,6 ms; 1 op 43 ms | 89 ms; 12,5 ms; 41 ms (con constantes de etiqueta fijadas en registros) |
| Tamaño de código | 62–78 B/op | 61–80 B/op |
| Binario | **+4,9 MB** (5,22 MB frente a 0,36 MB, x86-64 solo, stripped) | estimado +50–100 KB |
| Dependencias | **+38 crates** (`cranelift-*`, `regalloc2`, `gimli`, `hashbrown`, `target-lexicon`, ISLE y generadores en build); hoy `crates/js` tiene **cero** | ninguna |
| Build limpia | +212 s de CPU (5:56 de pared con carga) | despreciable |
| Toolchain (sin C, zig como cc/linker) | Rust puro; `cargo check --target x86_64-pc-windows-gnu` OK (1:26) | Rust puro |
| Arquitecturas | x86-64, aarch64, riscv64, s390x; ABIs SysV, fastcall de Windows y Apple aarch64 resueltos | x86-64 SysV + Win64 primero; aarch64 después (~1,5–2 k líneas, codificación fija de 32 bits) |
| W^X y liberar código | `cranelift-jit` no libera funciones sueltas (solo el módulo entero) → usar `cranelift-codegen` directo y nuestra propia memoria (igual que el propio) | nuestra memoria |
| Unwind en Windows | genera `UNWIND_INFO` de Win64 (`create_unwind_info`) | prólogo fijo ⇒ `UNWIND_INFO` constante (~60 líneas) |
| Mantenimiento | API con cambios incompatibles frecuentes (en el prototipo: `MemFlags` internados, `finalize(config)`, `icmp_imm` renombrado); bugs ajenos difíciles de depurar | bugs de codificación nuestros; mitigación: tests de bytes por plantilla, volcados con mnemónicos, tests diferenciales |
| Depuración | IR legible (CLIF), pero sin desensamblador aquí (no hay `objdump`) | el emisor puede producir su propio listado con mnemónicos por instrucción |

**Preferencia del propietario.** Cranelift es un generador de código, no un
motor JS: toda la semántica JS seguiría siendo nuestra, así que no viola la
regla de "sin motor JS de terceros". Pero metería ~200 k líneas ajenas en el
núcleo de un crate que hoy no tiene dependencias, y para un nivel base (helpers
+ plantillas, sin asignación de registros entre instrucciones) su optimizador y
su regalloc no aportan: el código sale igual de rápido y compila 500× más
lento. La preferencia y los datos apuntan al mismo sitio.

**Decisión: generador propio** para Level 1. Reevaluar Cranelift solo para un
futuro nivel optimizador (especialización de tipos, valores en registros,
OSR/deopt), donde su regalloc sí importa y el coste de compilación se amortiza
con umbrales altos (y, si se diseña así, compilando en otro hilo a partir de
una copia `Send` del bytecode).

---

## 4. Opcodes de la primera versión

Principio: **función entera o nada**. El generador hace un `match` exhaustivo
sobre `Op` **sin brazo comodín**, de modo que añadir un opcode al enum obliga a
clasificarlo. Tres clases:

### 4.1 Fast path nativo (con caída al helper si los tipos no encajan)

- Movimientos: `Mov` (convierte `Empty` en `undefined`), `MovRaw`, `LoadK`,
  `MovChecked`, `CheckInit` (si es `Empty`, helper que lanza el
  `ReferenceError`). Fast path solo si ni el valor nuevo ni el que se
  sobrescribe tienen contador; si no, helper `jit_mov`.
- Aritmética sobre números: `AddRR/RK` (la concatenación de strings va al
  helper), `SubRR/RK`, `MulRR/RK`, `DivRR/RK`, `ModRR/RK` (llamada a la función
  pura `js_mod`), `BinRR/RK` (`&`, `|`, `^`, `<<`, `>>`, `>>>` con ToInt32
  inline en rango, `**` vía la función pura `js_pow`), `IncR`, `DecR`,
  `ToNumericR`, `UnR` para `Neg`, `Plus`, `BitNot` (`typeof`, `ToString`,
  `ToPropertyKey` → helper), `NotR` (veracidad inline para
  bool/número/undefined/null; strings y resto → helper puro). Todo resultado
  numérico pasa por la **misma canonicalización de NaN** que `set_number`.
- Comparaciones: `CmpRR/RK` (`<`, `<=`, `>`, `>=`, `==`, `!=` entre números;
  `===`/`!==` entre números o bits idénticos que no sean strings; resto →
  helper puro `strict_equals`).
- Saltos: `Jump` (tick en saltos hacia atrás), `JumpUnless{Lt,LtEq,Gt,GtEq}{RR,RK}`,
  `JumpIf{Lt,LtEq,Gt,GtEq}{RR,RK}`, `JumpIfStrict{Eq,NotEq}{RR,RK}`,
  `JumpIf{True,False,Nullish,NotNullish,Undefined,NotUndefined}R`,
  `IncJumpIfLt{RR,RK}` (ronda 4) y **también las formas de pila** que queden
  (`JumpIfFalse`, `JumpIfTrue`, `JumpIfNullish`, `JumpIfNotNullishKeep`,
  `JumpIfFalseKeep`, `JumpIfTrueKeep`, `JumpIfUndefined`, `JumpIfNotUndefined`,
  `JumpIfStrictEq`) sobre el slot `nregs + depth - 1`. Ningún salto pasa por
  `exec_op` (que escribe `frame.pc`).
- `Nop`, `Debugger` (sin efecto, como en el intérprete).

### 4.2 Helper dedicado (sin fast path en Level 1)

- `ReturnR`, `Return` → `jit_return` (estado `RETURN` o `EXIT`).
- `CallR`, `Call`, `CallSpread`, `NewR`, `New`, `NewSpread`, `SuperCall`,
  `SuperCallSpread` → `jit_call` (estado `NEXT` si fue nativa o se resolvió
  dentro, `CALL` si se empujó un frame JS: punto de reanudación en `pc+1`).
- `Throw`, `ThrowConst` → estado `THROW`.
- `GetPropR`, `SetPropR`, `DefinePropR`, `GetGlobalR`, `SetGlobalR`: helpers que
  ejecutan **el mismo código de fast path de IC** que `run_inner` (se extrae a
  funciones `#[inline]` compartidas; no se duplica) y, si falla, `exec_op`.
- `GetElemR`/`SetElemR` (`get_elem_fast`/`set_elem_fast` y luego `exec_op`),
  `GetLocalR`/`SetLocalR`, `LoadThis`, `StoreCompletionR`.
- **Todo lo demás** (instrucciones de pila de propiedades, literales, closures,
  clases, iteradores, for-in, `PushScope/PopScope/CopyScope`, `typeof`,
  `delete`, `in`, `instanceof`, `arguments`, `ArgsLength/ArgsGet`, plantillas,
  RegExp, `super`, `with`, `import.meta`, `import()`…) → helper genérico
  `jit_op(ctx, pc)`, que llama a `exec_op` con el `pc` sincronizado. El JIT
  nunca cachea `frame.scope`, así que las instrucciones de scope son seguras
  vía helper.

### 4.3 Hacen que la función NO se compile (se queda en la VM)

- `flags.is_generator || flags.is_async` (y, por validación, cualquier
  `Yield`, `AsyncYield`, `Await`, `IteratorNextWith`).
- `CallEval` (eval directo: puede declarar `var` en el scope de la función;
  se deja fuera hasta tener tests diferenciales específicos).
- Funciones cuyo código quedó sin traducir a registros (`untranslated`, frames
  > `u16`), con más de `JIT_MAX_OPS` (p. ej. 20 000) instrucciones o con
  `frame_size` por encima de un límite.
- Fallo de validación: algún operando de slot ≥ `frame_size`, destino de salto
  ≥ `ops.len()`, índice de constante/IC/función fuera de rango, handler mal
  formado, o `depths` inconsistente.
- Excluida por `RABBITHOLE_JIT_EXCLUDE` / no incluida en `RABBITHOLE_JIT_ONLY`.

Con umbral 0 también se compilan cuerpos de script y de módulo (cobertura para
tests); con el umbral por defecto no llegan nunca (se ejecutan una vez).

---

## 5. Convención de llamada JIT ↔ VM ↔ runtime

### 5.1 Contexto y firma

```rust
#[repr(C)]
pub struct JitCtx {
    slots: *mut Value,          // +0  base de los slots del frame (vstack + base); la reescriben los helpers
    consts: *const Value,       // +8  code.constants (inmutable mientras corre)
    realm: *mut Realm,          // +16
    helpers: *const JitHelpers, // +24 tabla de punteros a función: el código no lleva relocaciones
    tick: *mut u32,             // +32 &realm.tick
    fi: u32,                    // +40 índice del frame
    pc: u32,                    // +44 pc de salida / reanudación que dejan los helpers
}

// Entrada de una función compilada (ABI C de la plataforma: SysV o Win64).
type JitEntry = unsafe extern "C" fn(ctx: *mut JitCtx, pc: u32) -> u32; // devuelve un estado

// Helpers por instrucción: decodifican ellos mismos `code.ops[pc]`.
type OpHelper = unsafe extern "C" fn(ctx: *mut JitCtx, pc: u32) -> u32;
// Helpers puros (sin Realm): js_mod, js_pow (f64, f64) -> f64; truthy(u64) -> u32; strict_eq(u64, u64) -> u32
```

Estados: `NEXT=0` (seguir), `CALL` (se empujó un frame JS; reanudar en `pc+1`
cuando vuelva), `RETURN` (frame sacado y resultado escrito en el llamador),
`EXIT` (volvió un frame `entry`; valor en `realm.jit_exit`), `THROW`
(excepción en `realm.jit_exc`, raíz del GC), `PANIC` (pánico capturado en un
helper; carga útil en `realm.jit_panic`).

### 5.2 Contrato de los helpers

1. Antes de nada que pueda lanzar o llamar, `frames[fi].pc = pc + 1` (igual que
   `sync!`), para que `unwind` (que mira `pc - 1`), las trazas de pila y
   `exec_op` (que usa `depths[pc - 1]`) vean exactamente lo mismo que con el
   intérprete.
2. Al volver con `NEXT`, `ctx.slots` es válido (recalculado desde `vstack`).
3. **Nunca desenrollan**: el cuerpo va dentro de `catch_unwind`; un pánico se
   guarda y se devuelve `PANIC`; la VM hace `resume_unwind` cuando ya no hay
   frames JIT en la pila. (Sin esto, un pánico que cruza un `extern "C"`
   aborta el proceso, y en Windows el desenrollador no sabría cruzar el frame
   JIT.)
4. Reutilizan código existente (`exec_op`, `try_fast_call`, `try_native_call`,
   `ic_get`/`ic_set`, `do_return`); **no se duplica semántica JS**.

### 5.3 Entrada, reanudación y salida

- La VM decide en `run_inner`, en `'reload` (todos los caminos que empujan un
  frame pasan por ahí: llamada rápida, general, nativo→JS, constructores):
  - `pc == 0`: incrementar el contador de la función; si supera el umbral y la
    compilación (cacheada en `FunctionCode`) tuvo éxito, marcar
    `frame.jit = true` y llamar a la entrada con `pc = 0`;
  - `frame.jit && pc > 0`: es una reanudación tras una llamada que empujó un
    frame; entrar con ese `pc`.
- El código generado empieza con un prólogo fijo y un **despacho de entrada**:
  `pc == 0` cae a la primera instrucción; si no, salto indirecto por una tabla
  densa de desplazamientos por instrucción (4 B/op) con comprobación de rango.
- Llamadas JS→JS: `jit_call` usa `try_fast_call`/`do_call`; si empujó un
  frame devuelve `CALL`, el código JIT retorna a la VM, que ejecuta el callee
  (interpretado o compilado); el `ReturnR` del callee escribe el resultado en
  `ret_slot` y recarga el frame del llamador, que se reanuda en `pc+1`.
  **Ninguna recursión nativa**: los límites de profundidad (`max_frames`) y el
  consumo de pila nativa son idénticos con y sin JIT. Esto no es deopt ni OSR:
  no se reconstruye estado y una activación compilada nunca se ejecuta en el
  intérprete salvo tras una excepción capturada (5.4).
- JS→nativo: dentro del helper (`try_native_call`/`call_native`), sin volver
  a la VM. Nativo→JS: como hoy (`Realm::call` → `run()` anidado), y ese `run()`
  puede a su vez entrar en código JIT.
- Integración en la VM (esquema):

```rust
#[cfg(feature = "jit")]
if let Some(entry) = self.jit_entry_for(fi) {
    match self.run_jit(fi, entry) {           // prepara JitCtx y llama
        JitExit::Reload => continue 'reload,   // CALL o RETURN
        JitExit::Return(v) => return Ok(Exit::Return(v)),
        JitExit::Throw(e) => return Err(e),    // run() → unwind(), como hoy
    }
}
```

### 5.4 Excepciones en código JIT

`THROW` llega a `run()` como cualquier `Err`; `unwind` usa la tabla de
handlers existente. Si el handler está en el propio frame compilado, **Level 1
borra `frame.jit` y el resto de esa activación sigue en el intérprete** (la
VM gestiona catch/finally, como pide la especificación). La tabla de entrada
permitiría reanudar en el `target` del handler; queda como opción posterior
detrás de un flag, cuando los tests diferenciales de excepciones estén verdes.

### 5.5 Registros fijos (x86-64)

`rbx` = `JitCtx`, `r12` = base de slots (recargada de `[rbx]` tras cada
helper), `r13` = `TAG_MIN` (o etiqueta `Number` en `main`), `r14` = umbral de
valores con contador, `r15` = `consts`, `rbp` = cadena de frame pointers.
Temporales: `rax`, `rcx`, `rdx`, `rsi`, `rdi`, `r8`–`r11`, `xmm0`–`xmm5`. En
Win64: argumentos en `rcx`/`rdx`, 32 bytes de shadow space en cada llamada,
`rsi`/`rdi`/`xmm6`–`xmm15` son callee-saved (no se usan `xmm6+`). Pila alineada
a 16 en cada llamada. Las constantes de JS (`LoadK`, operandos `K`) se cargan
de la tabla de constantes vía `r15`, **nunca como inmediatos** (evita JIT
spraying con dobles elegidos por el atacante).

Plantilla de `AddRR d, a, b` con el `Value` de la ronda 4 (caminos lentos fuera
de línea):

```text
mov rax, [r12+a*8] ; cmp rax, r13 ; jae slow_i      ; ¿número?
mov rcx, [r12+b*8] ; cmp rcx, r13 ; jae slow_i
movq xmm0, rax ; movq xmm1, rcx ; addsd xmm0, xmm1 ; movq rax, xmm0
cmp rax, r13 ; jae nan_i                            ; canonicalizar NaN (raro)
mov rdx, [r12+d*8] ; cmp rdx, r14 ; jae slow_i      ; ¿el valor viejo tiene Rc?
mov [r12+d*8], rax
...
slow_i: mov rdi, rbx ; mov esi, i ; call [helpers.op] ; mov r12, [rbx]
        test eax, eax ; jnz exit_status ; jmp next_i
```

---

## 6. GC, excepciones y liberación de código

- **GC**: no móvil, sin barreras, raíz = todo `vstack`. Level 1 no guarda
  ningún `Value` en registros nativos entre instrucciones (solo temporales
  dentro de una plantilla), así que no necesita mapas de pila. Todo helper se
  trata como posible punto de GC y de reubicación de `vstack` (recarga de
  `r12`). Safepoints propios del JIT: saltos hacia atrás y llamadas
  decrementan `*tick`; a cero, `jit_safepoint` (GC, watchdog, `RH_JSPROF`).
  Si algún día la GC es móvil o generacional: los objetos ya se direccionan
  por id (no por puntero) y la recarga tras helpers ya está; habría que añadir
  barreras como llamadas a helper en los stores a objetos (que en Level 1 ya
  son helpers).
- **Contadores de referencias**: el código generado nunca copia ni sobrescribe
  valores con `Rc`; cuando aparece uno, helper (`Value::clone` / drop de Rust).
- **Excepciones**: estado `THROW`, la VM desenrolla (5.4). Pánicos: capturados
  en cada helper (5.2). Un fallo de hardware dentro de código JIT es un bug
  (como hoy cualquier error de memoria); en depuración, un manejador de
  `SIGSEGV` que diga en qué función JIT ocurrió.
- **Windows x64**: registrar cada función con `RtlAddFunctionTable`
  (`RUNTIME_FUNCTION` + `UNWIND_INFO` constante del prólogo fijo) para que
  depuradores, volcados de WER y perfiles ETW recorran los frames JIT; no se
  depende de ello para el control de flujo (ningún desenrollado cruza código
  JIT). `RtlDeleteFunctionTable` al liberar. `FlushInstructionCache` tras
  escribir.
- **Linux**: se mantienen frame pointers (el muestreador `RH_PROF` y gdb
  funcionan sin unwind info); un registro de rangos de código permite que
  `RH_PROF` simbolice como `jit:<función>`; opcionalmente un fichero
  `/tmp/perf-<pid>.map`.
- **Memoria W^X**: Level 1 empieza simple: un mapeo por función redondeado a
  página (`mmap` RW → copiar → `mprotect` RX; `VirtualAlloc` →
  `VirtualProtect`), **nunca RWX** (un test comprueba que no hay mapeos `rwx`).
  En aarch64 hará falta `__clear_cache`/`sys_icache_invalidate` y, en macOS,
  `MAP_JIT` + `pthread_jit_write_protect_np`. Si la memoria por redondeo
  importa (medido en la fase 6), arena por hilo en trozos con listas libres
  (el `Realm` es por hilo, así que la arena también).
- **Liberación**: `JitCode` (mapeo + registro de unwind) pertenece al
  `FunctionCode` compilado (`OnceCell<Option<Box<JitCode>>>`; `None` recuerda
  "no compilable" para no reintentar). Su `Drop` desmapea y desregistra. Es
  seguro porque una función solo se ejecuta mientras un `Frame` de la VM
  mantiene un `Rc<FunctionCode>`, y los frames JIT siempre tienen su `Frame`
  en la VM.

---

## 7. Plan de fases de Stage 2

### Validación común (bloquea cada fase)

- `cargo test --workspace --features jit` con `RABBITHOLE_JIT=off` y con
  `RABBITHOLE_JIT=on RABBITHOLE_JIT_THRESHOLD=0` (variables de entorno porque
  `cargo test` no pasa flags de CLI): mismo conjunto de tests verde
  (`crates/js`: 57 + 20 + 18; `crates/script`: 33 + 31; resto del workspace).
  Un script `scripts/jit-check.sh` ejecuta ambos modos y compara.
- test262 no está en el repo: opcionalmente, un subconjunto fijado ejecutado
  desde un checkout externo en `/tmp`, igual en ambos modos.
- Tests diferenciales: un ejemplo `jitdiff` ejecuta cada script de un corpus
  (tests, casos de `perf.rs`, scripts capturados de páginas) en dos realms
  frescos (VM y umbral 0) y compara valor de compleción, salida de consola,
  excepción (tipo, mensaje, línea) y un resumen del estado global. **cargo-fuzz
  no es viable aquí** (requiere nightly, libFuzzer en C++ y runtimes de
  sanitizers; solo tenemos Rust estable y zig): en su lugar, un generador
  aleatorio de programas en Rust puro (semilla registrada) con mezcla de tipos,
  bucles, llamadas, try/catch, getters con efectos, `valueOf`, concatenaciones,
  ejecutado en segundo plano.
- GC-stress: `RH_GC_STRESS=1` fuerza `gc()` en cada safepoint con intervalo de
  tick 1, con JIT activado y umbral 0.
- Seguridad de memoria: asserts de depuración (slots en rango, `ctx.slots`
  coherente con `vstack` tras cada helper, `frame.pc` coherente), página de
  guarda tras el código, test "sin `rwx`". ASan con nightly
  (`-Zsanitizer=address`) solo si enlaza con zig: se intenta una vez en la
  fase 1 y se documenta el resultado.
- Cualquier regresión de compatibilidad bloquea la fase.

### Benchmarks en cada fase

VM sin JIT, VM con JIT (umbral por defecto y 0), y QuickJS como referencia:
**zig cc compila QuickJS** (verificado: `quickjs.c` + `dtoa.c` + `libregexp.c`
+ `libunicode.c` + `cutils.c` con un `main` de 60 líneas, ~2 min, binario de
7 MB) — solo para comparar, en `/tmp`, **nunca** en el producto ni en el repo.
Medir: tiempo de compilación (ns/op y total por página), tiempo de ejecución,
calentamiento (tiempo por llamada en las primeras N invocaciones), memoria
(RSS y bytes de código JIT), scripts cortos frente a largos, carga de páginas
(fixture DOM local, Wikipedia, YouTube con `shot`). Añadir a `perf.rs` una
categoría "funciones calientes" (el bucle dentro de una función llamada cientos
de veces), que es lo único que Level 1 puede acelerar.

### Depuración

`RABBITHOLE_JIT_DUMP=<nombre>` (bytes + listado por instrucción con
desplazamientos y mnemónicos que escribe el propio emisor; aquí no hay
`objdump`), `RABBITHOLE_JIT_TRACE=1` (qué se compila, qué no y por qué,
tamaño y tiempo), `RABBITHOLE_JIT_EXCLUDE=<patrones>`,
`RABBITHOLE_JIT_ONLY=<patrones>`, `RABBITHOLE_JIT_MAX=<n>` (compilar solo las
primeras n funciones: bisección automática de un miscompile).

### Fases

1. **Infraestructura** (sin compilar nada): feature `jit` en `crates/js`
   (propagada desde `app`), flags `--jit=off|on`, `--jit-threshold=N` y sus
   variables de entorno, `JitOptions` en `Realm`, campo `jit` en `Frame` y
   `FunctionCode`, contadores de invocación en la entrada de frame, memoria
   ejecutable W^X (Linux y Windows) con `Drop`, registro de rangos, trazas y
   listas de exclusión, e informe `RABBITHOLE_JIT_STATS` (% de ops ejecutados en
   funciones por encima del umbral). **Hecho cuando**: compila con y sin la
   feature (Linux y cross-build de Windows); con JIT desactivado la suite
   `perf.rs` y una carga de página no se mueven más allá del ruido (±1 %); un
   test escribe un stub `mov eax, 0; ret`, lo protege RX, lo ejecuta y lo
   libera, y comprueba que no existe ningún mapeo `rwx`; el informe de
   cobertura existe para `perf.rs`, el fixture DOM y YouTube. **Aquí se evalúa
   la puerta de la sección 2.8.**
2. **Funciones triviales**: emisor x86-64 (SysV + Win64), prólogo/epílogo,
   despacho de entrada, pasada de validación, `LoadK`, `Mov*`, aritmética
   RR/RK, `IncR/DecR`, `CmpRR/RK` sobre números y `ReturnR` (helper);
   cualquier otro op ⇒ la función no se compila. **Hecho cuando**:
   `function f(a, b) { return a * b + 1 }` y similares compilan y dan el mismo
   resultado con números y con no-números (caída al helper); paridad de la
   suite en umbral 0; volcados funcionando; ≤ 0,2 µs/op.
3. **Flujo de control**: todos los saltos (registro y pila), ticks en saltos
   hacia atrás y `jit_safepoint`, latches fusionados. **Hecho cuando**: bucles
   dentro de funciones calientes correctos; un bucle infinito en una función
   compilada es cortado por el watchdog; GC-stress verde; el bucle de 7 ops en
   función caliente ≥ 2× frente a la VM.
4. **Valores dinámicos**: fast paths por tipo con caída al helper
   (veracidad, nullish/undefined, igualdad estricta, ToInt32 en operaciones de
   bits, movimientos con conciencia de `Rc`), y el helper genérico `jit_op`, que
   habilita todas las instrucciones que no son llamadas. **Hecho cuando**: el
   informe muestra que casi todas las funciones no generadoras compilan (cifra
   de cobertura publicada), y el fuzzer diferencial pasa 10⁶ programas sin
   diferencias.
5. **Llamadas y excepciones**: `jit_call` con puntos de reanudación (JS→JS por
   la VM, JS→nativo en el helper, nativo→JS por `run()`), `jit_return`, `Throw`,
   estado `THROW` y desenrollado por la VM, `PANIC` con `catch_unwind`,
   registro de unwind en Windows. **Hecho cuando**: toda la suite (incluidos
   los tests de `crates/script`/DOM) pasa en umbral 0; la profundidad máxima de
   recursión es idéntica con y sin JIT; mensajes y trazas de excepción
   idénticos; cross-build de Windows verde (y prueba de humo en una máquina
   Windows en cuanto haya una; hasta entonces, JIT desactivado en Windows).
6. **Tiering automático**: umbral por defecto ajustado con datos (probable
   500–2000 llamadas), fallos de compilación cacheados, límite de memoria JIT
   por realm (p. ej. 16 MB, después se deja de compilar), test de liberación
   (crear y soltar miles de closures: los bytes JIT vuelven a la línea base),
   arena por trozos si el redondeo a página pesa. **Hecho cuando**: con JIT
   activado las cargas de página no empeoran más del 1 % de CPU, se cumplen
   los objetivos de la sección 9, y el JIT sigue **desactivado por defecto**
   hasta que el propietario decida.

---

## 8. Riesgos y mitigaciones

| Riesgo | Mitigación |
|---|---|
| Poco retorno (medido: ≤ 2–4 % en páginas) | Puerta de decisión tras la fase 1; hacer antes A–C (sección 2.7) |
| Punteros obsoletos a `vstack` tras una reubicación | Recarga obligatoria de `r12` tras cada helper; asserts en depuración; los helpers recalculan `ctx.slots` |
| Fugas o doble liberación de `Rc` | El código generado nunca copia/sobrescribe valores con contador |
| Pánicos cruzando frames JIT (aborto, o desenrollado roto en Windows) | `catch_unwind` en todos los helpers + `resume_unwind` en la VM |
| Divergencia semántica entre fast paths y VM | Fast paths mínimos; todo lo demás reutiliza `exec_op` y los fast paths de IC compartidos; tests diferenciales y fuzzer |
| NaN fuera de canon dentro del espacio de etiquetas (un sNaN `0xFFF7…` silenciado da `0xFFFF…`) | Canonicalizar todo resultado numérico igual que `set_number` |
| La ronda 4 cambia `Value` u opcodes | Módulo `value_abi` aislado con asserts de tamaño y discriminantes; `match` exhaustivo sin comodín |
| Windows sin máquina de prueba | Solo cross-build verificable aquí; JIT desactivado en Windows hasta una prueba de humo real |
| Seguridad (JIT spraying, W^X, validación) | Constantes desde memoria, nunca RWX, bytecode validado antes de emitir, bounds checks del runtime intactos |
| Coste de mantener dos motores | Clasificación obligatoria de opcodes nuevos, script de paridad en CI, exclusiones por función para aislar bugs |
| Generadores/async quedan fuera | Los puntos de reanudación ya existen: en una fase futura un `Yield` puede ser "volver a la VM con estado" sin cambiar el modelo |
| Memoria de código (página por función) | Medir en la fase 6; arena por trozos si hace falta; límite por realm |

---

## 9. Objetivos numéricos (ajustados al perfil)

| Métrica | Objetivo Level 1 | Referencia medida |
|---|---|---|
| Función caliente con bucle numérico de varias instrucciones | **≥ 2×** frente a la VM | prototipo 2,7× |
| Bucle de una sola instrucción fusionada | ≥ 1,2× | prototipo 1,2–1,3× |
| Llamadas (fib, method_calls) | ≥ 1,0× (sin regresión); la ganancia de verdad viene de B | `CallR`/`ReturnR` siguen en la VM |
| `perf.rs` total con umbral por defecto | sin regresión (±1 %) | casi todo es código que corre una vez |
| Carga de páginas y scripts que no calientan, JIT activado | ≤ 1 % de CPU de regresión (la guía decía ≤ 3 %; con 0,06 µs/op cabe en menos) | — |
| JIT compilado pero desactivado | 0 % medible | una carga y un salto por entrada de frame |
| Compilación | ≤ 0,2 µs/op, ≤ 100 B/op | prototipo 0,06–0,12 µs/op, 61–80 B/op |
| Código JIT en una página pesada (YouTube) | ≤ 4 MB | a medir en la fase 6 |

El "≥ 2× en benchmarks numéricos y de bucles" de la guía solo es alcanzable en
la categoría "funciones calientes": los bucles actuales de `perf.rs` están en
código de una sola ejecución y Level 1 (sin OSR) no los toca.

---

## 10. Qué cambia si la ronda 4 aterriza o no

- **Si aterriza** (lo más probable): `Value` de 8 bytes: comprobar número = una
  comparación sin signo con `TAG_MIN`; con contador = `>= 0xFFFC…`; constantes
  inmediatas para undefined/null/empty/true/false; un solo store de 8 bytes por
  valor; canonicalización de NaN obligatoria. Los ops fusionados
  `IncJumpIfLtRR/RK` entran en 4.1. Los nuevos ICs (prototipo profundo,
  `IC_ABSENT`, megamórfica) y los ICs de llamada se aprovechan sin hacer nada,
  porque las propiedades y las llamadas van por helpers que reutilizan esos
  caminos. Si las "globales más baratas" introducen opcodes nuevos, el `match`
  exhaustivo obliga a clasificarlos (por defecto, helper genérico).
- **Si no aterriza**: `Value` de 16 bytes con `#[repr(u64)]`: número =
  `qword [slot] == 3`; con contador = etiqueta 4 o 6; guardar un número son dos
  stores de 8 bytes (etiqueta y payload; nunca stores parciales, que rompen el
  store-to-load forwarding, como ya documenta `value.rs`). El intérprete de
  `main` es más lento (empty_loop 71 ms frente a 51), así que el speedup
  relativo del JIT sería algo mayor (estimado ~1,7× en el bucle simple: 71 ms
  frente a ~41 ms), pero
  la conclusión de 2.8 no cambia.
- En ambos casos, todo lo que depende del layout vive en un único módulo
  `value_abi` (≈ 100 líneas: es-número, unbox/box f64, tiene-contador,
  constantes), con tests que comprueban `size_of::<Value>()` y discriminantes.

---

## Apéndice A. Reproducir las medidas

- Binarios con símbolos y frame pointers:
  `CARGO_PROFILE_RELEASE_STRIP=false RUSTFLAGS=-Cforce-frame-pointers=yes cargo build --release -p rabbithole-js --example perf --example prof`
  (y `-p rabbithole-engine --example shot` para páginas; se usó
  `CARGO_PROFILE_RELEASE_LTO=false` para acortar la build).
- Perfiles: `RH_PROF=out.prof target/release/examples/perf <caso>` y
  `RH_PROF=out.prof RH_JS_WAIT=6000 target/release/examples/shot <url> x.png`;
  simbolizar leyendo `.symtab` del binario y desplazando por la dirección de
  `rh_sampler_marker` registrada en la primera línea; `out.maps` sirve para
  atribuir direcciones de libc. Para separar "dentro de JS", quedarse con las
  muestras cuya cadena contiene `run_inner`.
- Atribución por opcode (no comiteada): en `run_inner`, tras leer `op`, guardar
  su primer byte (discriminante) en una `AtomicUsize`; en `run()`, guardar y
  restaurar ese valor alrededor de `run_inner` (para los `run()` anidados);
  en el manejador de SIGPROF, añadir ese valor a la muestra. Con
  `RH_OPCOUNT=<fichero>`, un contador por discriminante da las frecuencias.
- Prototipo JIT: un crate desechable con `cranelift-codegen`/`cranelift-frontend`
  0.137 y un emisor x86-64 propio sobre el mismo mini-bytecode; tiempos con
  `CLOCK_THREAD_CPUTIME_ID`; código en `mmap` RW → `mprotect` RX.
- QuickJS: `zig cc -O2 -fwrapv -D_GNU_SOURCE -DCONFIG_VERSION=... runner.c quickjs.c dtoa.c libregexp.c libunicode.c cutils.c -lm -lpthread`,
  con un `runner.c` que evalúa los mismos 21 casos en contextos nuevos.

---

## Apéndice B. WebAssembly sobre el mismo emisor (`crates/wasm`)

El intérprete de WebAssembly (`crates/wasm`, sin JIT por ahora) se diseñó
para que un futuro nivel nativo reutilice el emisor de plantillas propio de
la sección 3 en lugar de añadir un segundo backend:

- **Mismo modelo de máquina.** El IR de wasm (`crates/wasm/src/ir.rs`) es de
  tres direcciones sobre *slots* de 64 bits relativos al puntero de frame:
  `I32Add a ← b, c` lee `[fp + b*8]` y `[fp + c*8]` y escribe `[fp + a*8]`.
  Es exactamente la forma que consumen las plantillas de Level 1 (operandos
  en memoria del frame, sin asignación de registros entre instrucciones), así
  que cada `Instr` se traduce a una plantilla corta: carga, operación,
  almacenamiento. `local.get` y las constantes ya no generan movimientos (el
  consumidor lee el slot del local o el slot de constante), y las
  comparaciones seguidas de `br_if`/`if` ya vienen fusionadas
  (`BrI32LtS a, b → destino`), que es lo que una plantilla de salto
  condicional quiere.
- **Sin comprobaciones de tipo.** A diferencia de JS no hay fast path ni
  helper de caída: los tipos son estáticos (validados), así que cada opcode
  numérico es su plantilla completa. Las únicas salidas al runtime son
  llamadas a funciones host, `memory.grow`, operaciones de tabla/bulk y
  traps; todas pueden ir por helpers con el mismo contrato de la sección 5.2
  (contexto en registro fijo, resultado de error en el retorno).
- **Memoria lineal.** El intérprete guarda base y longitud de la memoria 0 en
  variables locales y las recarga tras `memory.grow` o llamadas al host; el
  código nativo haría lo mismo con dos registros fijos (base, límite), con
  comprobación de límites explícita `ea + n > len` (no se usan páginas de
  guarda: no hay manejador de señales propio en Windows).
- **Llamadas.** Convención sin copia: los argumentos quedan en los slots
  superiores del llamante y el frame del llamado empieza ahí; los resultados
  vuelven en los slots `0..n` del llamado. Un JIT conserva la convención (el
  `fp` del llamado es `fp + base*8`), de modo que código nativo e intérprete
  pueden llamarse entre sí frame a frame.
- **Puntos seguros.** Los saltos hacia atrás y las llamadas decrementan un
  contador y consultan el watchdog del realm (`Realm::check_watchdog`) al
  llegar a cero; el código nativo emitiría el mismo decremento en cada
  arista de vuelta.
- **Excepciones.** Las tablas de handlers son rangos de índices de
  instrucción; con código nativo se convierten en rangos de PC nativo
  (misma búsqueda), sin unwind de la pila nativa: el frame de wasm vive en la
  pila de valores, no en la nativa.
- **Medidas del intérprete** (`crates/wasm-js/examples/wasm_bench.rs`,
  máquina cargada, ms de pared): `fib(30)` 393 frente a 1092 del mismo
  código en nuestro JS; `matmul(128)` f64 392 frente a 780; compresión tipo
  SHA-256 de 4096 bloques 440 frente a 2294. Es decir, 2–5× más rápido que
  el JS interpretado sin JIT; una plantilla nativa por `Instr` eliminaría
  el coste del dispatch, que domina en estos bucles.
- **Nivel base nativo (hecho, `crates/wasm/src/jit.rs`).** Para wasm el
  emisor de plantillas ya existe: x86-64 (System V en todas las
  plataformas vía `extern "sysv64"`, también en Windows), una plantilla por
  `Instr` sobre `[rbx + slot*8]` con rbx = frame, r12/r13 = base/longitud
  de la memoria 0, r14 = contexto. Las llamadas entre funciones compiladas
  de la misma instancia son `call` nativas (rbx avanza al frame del
  llamado); cualquier otra llamada, las instrucciones sin plantilla y los
  caminos lentos (posibles traps) llaman a helpers que ejecutan el código
  del propio intérprete para esa instrucción (`exec_simple`), así que la
  semántica rara es la del intérprete. Los errores se devuelven como
  estado 1 y cada frame nativo retorna. Las funciones con handlers de
  excepción o tail calls siguen interpretadas. Con el parser SWC de
  rollup (playground de Svelte): 3,14 s de CPU interpretado frente a
  0,64 s nativo (≈5×); toda la suite de especificación pasa igual. En el
  playground completo (página real, worker de bundling) el bundle de
  hello-world tarda 6,5 s con el intérprete y 4,2 s con el nivel nativo; el
  resto del tiempo hasta la vista previa (≈17 s desde la carga) es
  JavaScript, no wasm.

---

## Stage 2: implementación (estado)

Aprobado por el propietario ("todo lo que mejore cobertura, performance o
fixes es un OK"). Hecho en este orden:

### S2.0 Reestructuración del bucle del intérprete

`run_inner` se partió para que `pc`, la base de slots y el código vivan en
registros: el bucle (`run_loop`) no devuelve nada (su `JsResult<Exit>` va a
`Realm::loop_end`; el puntero oculto de retorno ocupaba un registro), las
instrucciones se emparejan por referencia (copiar el `Op` decodificaba todos
los campos antes del salto de dispatch), el índice de frame, el
desplazamiento de slots y la instrucción actual ya no viven a lo largo del
bucle (los caminos fuera de línea los recalculan y releen `ops[pc-1]`), y
los pasos fuera de línea son funciones `#[inline(never)]` que devuelven un
`Ctl` de un byte. `perf.rs`: 750 → 712 ms (−5 %; `global_loop` −15 %,
`switch` −10 %, `calls`/`fib` −6 %). Speedometer (subconjunto, 6 pares
alternos): neutro dentro del ruido (±3 %).

### S2.1 Desviaciones respecto al diseño (y por qué)

- **Un solo convenio de llamada**: código nativo y helpers usan
  `extern "sysv64"` en todas las plataformas x86-64 (como el JIT de wasm);
  Windows no necesita una segunda variante de plantillas.
- **Entrada en cualquier frontera de instrucción**: como todo valor vive en
  el frame, la tabla de entrada permite entrar en cualquier `pc` (no solo
  0 y puntos de continuación). Eso da gratis la reanudación tras llamadas,
  la entrada en manejadores de excepción (no hace falta volver al
  intérprete tras un `catch`) y el OSR de bucles (S2.7).
- **Cambio de frame sin salir del código nativo**: tras una llamada que
  empuja un frame JS o un `return`, el helper `h_switch` reescribe el
  contexto para el nuevo frame y salta a su entrada interna (todas las
  funciones comparten el mismo prólogo, así que el frame nativo es el
  mismo): no hay recursión nativa (la profundidad máxima es la del
  intérprete) y una llamada JS→JS entre funciones compiladas no pasa por el
  bucle del intérprete.
- **Propiedad del código**: el código nativo pertenece a su
  `FunctionCode`; `run_script` mantiene su `Rc` durante todo el `run()` (un
  `return` del frame de script soltaba el último `Rc` mientras su código
  nativo aún ejecutaba el epílogo).
- **Coste cero con el JIT apagado**: `run_loop` es genérico sobre un
  `const JIT: bool`; con el JIT apagado se ejecuta la instancia sin
  ganchos, idéntica a un build sin la feature.
- **Fast paths de acceso**: los helpers genéricos eran más lentos que los
  fast paths del intérprete (helpers con `catch_unwind`, sincronización de
  `pc` y recálculo de la base). Se añadieron helpers de acierto de IC sin
  sincronización (globales, propiedades, elementos, concatenación) y
  plantillas nativas para `GetGlobalR`/`SetGlobalR` (`IC_GLOBAL_LEX` e
  `IC_GLOBAL_OBJ`, leyendo `PropIc` — ahora `#[repr(C)]` — y la época de
  prototipos directamente).

### S2.2 Fases

Cada fase es un commit pequeño y revertible: el compilador entero está en
`crates/js/src/jit/emit.rs` y `emit::PHASE` decide qué instrucciones admite
(las de fases posteriores hacen la función no compilable); el OSR de bucles
está detrás de `jit::LOOP_OSR`.

| Fase | Contenido | Validación |
|---|---|---|
| 1 | crate `rabbithole-jit` (memoria W^X, ensamblador con listados), feature `jit`, flags, contadores, estadísticas, trazas, exclusiones, `jitdiff`, `jit-check.sh` | compila con y sin feature (Linux, cross-check Windows); stub `mov eax,0; ret` RX y sin mapeos `rwx`; JIT apagado dentro del ruido |
| 2 | funciones en línea recta: `LoadK`, `Mov*`, aritmética RR/RK, `CmpRR/RK`, `ReturnR` | tests diferenciales, suite JS a umbral 0 |
| 3 | saltos (registro), latches fusionados, ticks en saltos hacia atrás | watchdog corta un bucle infinito nativo; GC-stress |
| 4 | helper genérico (`h_exec`) para todas las instrucciones que no son llamadas, accesos con IC, globales nativas, veracidad/nullish/igualdad estricta | fuzzer diferencial |
| 5 | `CallR`/`NewR`/llamadas de pila, `ReturnR`, `Throw`, excepciones (la VM desenrolla; el manejador puede estar en código nativo), pánicos capturados en helpers, cambio de frame nativo | suites `js`, `script`, `frameworks` a umbral 0; profundidad de recursión idéntica |
| 6 | umbral, memoria de código por hilo (16 MB), liberación con la función, coste de compilación medido | test de liberación (miles de funciones) |
| 7 | OSR de bucles | `perf.rs` con el JIT activado |

### S2.3 Validación

- Suites con `RABBITHOLE_JIT=on` y umbral 0 (todo se compila en la primera
  ejecución) y umbral 2 (los bucles entran en código nativo a mitad de
  ejecución): `js` 137/137, `script` 76/76, `frameworks` 66/66, iguales que
  con el JIT apagado (`scripts/jit-check.sh` compara ambos modos).
- Tests diferenciales (`tests_jit.rs`): aritmética con NaN/−0/enteros
  grandes, NaN con carga útil fuera de canon (Float64Array), saltos,
  veracidad, cadenas y BigInt (contadores de referencias), getters y
  `valueOf`, excepciones capturadas y no capturadas (mismo mensaje),
  recursión máxima idéntica, watchdog sobre un bucle nativo infinito,
  liberación del código, y GC-stress (`RH_GC_STRESS=1`: GC en cada safe
  point, tick 1).
- Fuzzer diferencial (`examples/jitdiff`, generador propio con semilla):
  más de 40 000 programas aleatorios + el corpus de `perf.rs`, 0
  diferencias. Encontró un fallo del intérprete (no del JIT): `a = a + a`
  en bucle desbordaba la longitud de la cuerda y hacía pánico; ahora es
  `RangeError: Invalid string length` como en V8.
- W^X: test del stub y comprobación de que no existe ningún mapeo `rwx`.
- Windows: `cargo check --target x86_64-pc-windows-gnu` verde; el código
  nativo sigue desactivado en Windows hasta probarlo en una máquina real.

### S2.4 Medidas (ms de CPU del hilo, mejor de 3, mínimo de 2 rondas alternas)

`perf.rs` (`main` = antes de Stage 2; VM = intérprete reestructurado; VM+JIT =
`RABBITHOLE_JIT=on`, umbral 1000, OSR; QuickJS 2025-04-26 compilado con
`zig cc -O2` fuera del repo, solo como referencia):

| caso | main | VM | VM+JIT | QuickJS | JIT/VM |
|---|---:|---:|---:|---:|---:|
| empty_loop | 45,9 | 45,9 | 36,7 | 91,8 | 1,25× |
| global_loop | 60,4 | 54,2 | 42,0 | 85,1 | 1,29× |
| local_loop | 19,5 | 18,0 | 14,4 | 22,2 | 1,25× |
| let_loop | 22,6 | 22,9 | 18,1 | 56,0 | 1,27× |
| props | 37,6 | 37,1 | 30,8 | 43,3 | 1,20× |
| array_rw | 34,6 | 33,7 | 28,2 | 65,4 | 1,19× |
| try_catch | 8,0 | 7,5 | 5,9 | 13,2 | 1,29× |
| switch | 35,8 | 34,0 | 19,3 | 50,7 | 1,76× |
| hot_7ops | — | 49,3 | 23,7 | 197,6 | 2,08× |
| hot_branchy | — | 40,7 | 25,2 | 33,5 | 1,62× |
| hot_bits | — | 93,9 | 67,2 | 98,2 | 1,40× |
| hot_array | — | 27,8 | 24,5 | 34,1 | 1,13× |
| calls / closure_calls / method_calls | 20,9 / 30,3 / 32,4 | 19,9 / 30,1 / 31,7 | 21,9 / 33,9 / 34,1 | 23,0 / 38,3 / 42,5 | 0,89–0,93× |
| new_objects / push_objects | 65,1 / 36,3 | 61,2 / 36,9 | 65,4 / 39,8 | 77,4 / 66,5 | 0,93–0,94× |
| strings / string_ops | 5,1 / 39,6 | 5,2 / 41,3 | 6,4 / 53,4 | 16,2 / 33,9 | 0,77–0,81× |
| for_in | 31,9 | 32,3 | 38,5 | 38,8 | 0,84× |
| json / array_map / arguments / fib | 49,8 / 56,3 / 27,9 / 17,7 | 50,6 / 56,5 / 27,5 / 16,7 | 52,0 / 57,1 / 27,8 / 17,9 | 96,7 / 53,0 / 162,2 / 12,7 | 0,93–0,99× |
| **TOTAL** (27 casos) | — | 1014 | 919 | 1607 | 1,10× |

- Compilación: 0,9–2,2 µs/op y ~30 µs por función, de los que ~22 µs son
  `mmap` + `mprotect` de una página por función (objetivo de diseño
  ≤ 0,2 µs/op no alcanzado por ese coste fijo; una arena de código lo
  quitaría). Código: ~70 B/op (objetivo ≤ 100).
- Memoria: en Speedometer se compilan 261 funciones (≈1,25 MB de código sin
  contar el redondeo a páginas); límite 16 MB por hilo.
- Speedometer 3.1 (TodoMVC-jQuery, Charts-chartjs, NewsSite-Next), mismo
  binario, 5 pares alternos: geomean con el JIT apagado 4605–5119 ms
  (mediana 4687), encendido 4823–5563 ms (mediana 4958): **≈ +5 %**, peor en
  Charts (+9 %). Con el JIT el 45–85 % de las muestras están en código
  nativo, pero hay 0,7–4 M de entradas en código nativo por suite: cada
  llamada desde código nativo a una función interpretada sale y vuelve a
  entrar (prólogo, despacho, epílogo, `jit_run`), y los helpers de
  acceso/llamada cuestan más que los fast paths inline del intérprete.

### S2.5 Estado y recomendación

- El JIT está completo según el diseño (fases 1–6) más OSR de bucles
  (fase 7), validado (suites a umbral 0 y 2, diferencial, fuzzer,
  GC-stress), y **sigue desactivado por defecto**: cumple ≥ 2× en el bucle
  numérico de varias instrucciones (hot_7ops 2,08×) y mejora 1,2–1,8× los
  bucles de `perf.rs`, pero en páginas reales cuesta ≈ 5 % (objetivo
  ≤ 3 %), y el código con muchas llamadas o métodos de string es 7–23 %
  más lento.
- Siguientes pasos, por retorno esperado: (1) entradas/salidas más
  baratas — llamar a funciones interpretadas sin salir del código nativo
  no es posible sin recursión, pero sí bajar la tasa: compilar antes los
  callees llamados desde código nativo y hacer `jit_run`/prólogo mínimos;
  (2) fast paths nativos de `GetPropR`/`SetPropR` (`IC_OWN`) y de `CallR`
  a funciones compiladas; (3) arena de código (coste de compilación);
  (4) prueba en Windows real para activarlo allí.

### S2.6 Fase 8: accesos y llamadas en código nativo (rendimiento)

Medido antes de tocar nada (Speedometer, perfiles `RH_PROF` JIT
apagado/encendido del mismo binario y contadores por instrucción en los
helpers): el código nativo en sí era solo ~2 % de las muestras; lo que
costaba eran los helpers (`h_call`, `h_return`, `h_get_prop`,
`step_access`, `h_exec`), que hacían lo mismo que los fast paths del
intérprete más una llamada. Las transiciones nativo→interpretado eran
pocas (≈ 140 k por suite; el contador `entries` incluía las llamadas
entre funciones nativas, ahora separadas como `native calls/returns`).
Por frecuencia en las tres suites: `GetLocalR` 7,2 M, `GetPropR`
8,1 M (4,7 M fallos de caché), `GetElemR` 4,4 M, `GetLocalChecked`
2,8 M, `Mov`/`MovRaw`/`LoadK` con strings 2,6 M (iban al helper genérico
por el contador de referencias), `LoadThis` 1 M, `DefinePropR` 0,9 M,
`SetLocalR` 0,8 M, `CmpRK` no numérico 0,7 M, `typeof` 0,6 M.

Cambios (todos en `crates/js/src/jit` salvo lo indicado):

- **Contadores de referencias en línea**: strings y BigInts se copian y
  sueltan en código nativo (`Rc`: el contador fuerte está 16 bytes antes
  del puntero; se comprueba una vez al arrancar y si no se cumple se
  vuelve a los helpers). Un `store` suelta el valor viejo; el último
  `Rc` lo libera `h_drop`.
- **Disposición de objetos sondeada en tiempo de ejecución**
  (`jit/layout.rs`): tabla de objetos, `PropertyMap` (sus valores en
  modo forma salen del enum: `values` vive en el struct, vacío en modo
  diccionario), prototipo (`Option<ObjectId>`), etiqueta del variante
  `Scope`/`Array` de `ObjectKind` (byte de etiqueta o nicho en la
  capacidad del vector del array, según cómo lo disponga rustc), frames.
  Se mide sobre instancias vivas, no se asume; un test exige que la sonda
  funcione. `RABBITHOLE_JIT_NO_INLINE_IC=1` lo desactiva.
- **En línea**: `GetPropR` (`IC_OWN`; `IC_PROTO`/`IC_PROTO_DEEP` fuera
  de línea), `SetPropR` (`IC_OWN`), `GetElemR`/`SetElemR` de arrays
  (índice entero en rango, sin huecos), `GetLocalR`/`GetLocalChecked`/
  `SetLocalR` (recorren la cadena de scopes), `LoadThis`, `==`/`!=` con
  null/undefined y entre tipos iguales (helper puro `h_loose_eq`),
  `typeof` (helper puro con strings compartidos). Helpers ligeros para
  `DefinePropR` y `StoreCompletionR`. Un fallo de un tipo de caché en
  línea va directo a la instrucción completa.
- **Cambio de frame**: un helper que empuja o saca un frame devuelve la
  dirección del código nativo del nuevo frame (una llamada en vez de
  dos); el gancho del bucle del intérprete está en línea y solo compila
  fuera de línea.
- **Intérprete** (`vm.rs`): un sitio de caché vacío adopta el registro
  megamórfico que lo resolvió. Antes, un sitio monomórfico cuya primera
  consulta acertaba en la caché compartida no se llenaba nunca y tomaba
  el camino lento siempre (p. ej. `Math.cos` en `canvas.js`: cada arco
  fallaba 9 veces).

Validación: `jit-check` (js, script, frameworks) a umbral 0 y 2
idéntico al intérprete; `jitdiff` 48 000 programas + GC-stress 300 + los
corpus nuevos de igualdad laxa/arrays/cachés (también como tests en
`tests_jit.rs`), 0 diferencias.

Medidas (mismo binario, JIT apagado/encendido alternos, mismo equipo,
bajo el `heavy.lock`):

| | antes (S2.4) | ahora |
|---|---:|---:|
| `perf.rs` total VM+JIT / VM | 1,10× | **1,27×** |
| peor caso de `perf.rs` | 0,77× (string_ops) | **0,96×** (push_objects; array_map 0,97–0,99) |
| props / prop_set / array_rw | 1,20 / — / 1,19× | 1,85 / 1,71 / 1,56× |
| closure_calls / method_calls / calls / fib | 0,89–0,93× | 1,22 / 1,25 / 1,06 / 1,04× |
| strings / string_ops / for_in | 0,77–0,84× | 1,17 / 1,05 / 1,04× |
| Speedometer (subconjunto), geomean JIT/VM | +5 % | **+1,3 … +2,1 %** (6–8 rondas) |
| ídem sin GC (`RH_NO_GC=1`) | — | **+1,0 %**; con `ENTRY_MIN` 24: −0,3 % |

- TodoMVC y NewsSite quedan en paridad (±1 %); las partes síncronas de
  Charts también (−1 … +1 %). Lo que queda en Charts está en las fases
  asíncronas («Draw opaque scatter/Async» 30 → 143 ms, «Show
  tooltip/Async» +135 ms) y no es CPU de JavaScript: con el JIT una
  recolección completa (≈ 600 k objetos, ~100 ms) cae dentro de la
  ventana medida en vez de después (`RH_GC_TRACE`: 15 colecciones frente
  a 14 con el mismo total de objetos liberados en la página aislada, que
  da imágenes idénticas y tiempos iguales con y sin JIT). Sin GC, Charts
  con JIT queda en 0,997×.
- `ENTRY_MIN_OPS` pasa de 48 a 24 (funciones sin bucle de ≥ 24
  instrucciones se ejecutan en nativo también desde llamadores
  interpretados): −0,3 … −0,8 % en Speedometer, `perf.rs` igual.
- Coste de compilación: ~190 B de código por instrucción (antes ~75; las
  plantillas de accesos en línea son largas), 1,4–1,7 µs/instrucción de
  los que la mitad es `mmap`+`mprotect` por función. Por suite son
  4–25 ms: la arena de código no se hizo; se pospone (necesitaría doble
  mapeo RW/RX para no tener que cambiar la protección de páginas que
  pueden estar ejecutándose).
- `tests_webapi::indexeddb_in_memory` falla a veces a umbral 0 en paralelo
  y con carga (3 de 5 en el equipo remoto, nunca solo): el test avanza
  tres turnos de temporizadores con rebanadas de tiempo real y, con todo
  compilado en la primera ejecución, no le da tiempo. No es una
  diferencia de semántica (a umbral 2 y sin carga pasa).

### S2.7 Estado

- Criterios para activarlo por defecto (`perf.rs` sin casos < 0,95× y
  Speedometer ≥ 0 %): `perf.rs` se cumple; Speedometer queda en +1–2 %
  con GC (el resto es la recolección desplazada de Charts) y en paridad
  sin GC. **El JIT sigue desactivado por defecto**; cambiarlo es una
  línea (`Options::on` en `jit/mod.rs`) y en Windows debe seguir
  apagado hasta probarlo en una máquina real.
- Siguientes pasos: (1) llamadas: `h_call`/`h_return` son ahora lo más
  caro del código nativo en páginas (≈ 15 % de su tiempo); un empuje de
  frame en línea dependerá de la disposición de `Frame`, que está
  cambiando; (2) despachar la entrada tras una llamada sin tabla (la
  entrada y el despacho son ~15 % del tiempo en código nativo);
  (3) arena de código con doble mapeo; (4) disparo de la GC menos
  sensible al momento (por bytes y objetos a la vez) para que mover el
  trabajo no mueva colecciones enteras dentro de las ventanas medidas.

### S2.8 GC incremental (pausas cortas)

Motivo: S2.6/S2.7 — una recolección completa caía dentro de las ventanas
medidas. Medido antes de tocar nada (`RH_GC_STATS=1`: número de pausas,
total, máximo, p95, histograma y tiempo por fase; mismo equipo, local,
con el asignador del navegador — `shot` usaba el del sistema, que es
~20 % más lento en Speedometer y no activa el disparo por bytes):

| | pausa máx. | p95 | GC total |
|---|---:|---:|---:|
| Speedometer (TodoMVC-jQuery, Charts-chartjs, NewsSite-Next) | 12–20 ms | 11–12,5 ms | ~420–480 ms |
| YouTube (portada, 15 s) | 47–69 ms | 37–54 ms | 235–300 ms |
| portada de un diario grande (15 s) | 29 ms | 24 ms | ~700 ms |

El barrido es más de la mitad del coste (liberar ~45 k objetos por ciclo).

Diseño (`heap.rs`, `realm.rs`):

- **Marcado incremental snapshot-at-the-beginning** (tricolor: blanco,
  gris, negro, y «negro débil» para WeakMap/WeakSet escaneados cuyas
  entradas esperan al paso de efímeros). Un ciclo agrisa las raíces de
  golpe (`gc_roots`, < 0,2 ms) y marca por pasos con presupuesto. Todo lo
  alcanzable al empezar, más todo lo asignado desde entonces (se asigna
  negro), sobrevive.
- **Barrera de escritura en `Heap::get_mut` / `try_get_mut`**: toda
  modificación de un objeto pasa por ahí (propiedades, elementos,
  variables de closures/scopes, prototipos, Map/Set, frames de
  generadores, wrappers DOM...), así que un objeto no escaneado se
  escanea antes de cambiar. Coste fuera del marcado: una comparación de
  un byte. Los dos sitios que escriben sin `get_mut` la llaman a mano:
  el store en línea de globales del IC (`ic_set_global`) y las entradas
  de WeakMap (sus valores no se trazan al escanear el mapa: `set`,
  `delete` y `getOrInsertComputed` agrisan el valor que se pierde,
  `Heap::shade_value`). Las raíces no llevan barrera: forman parte de la
  instantánea.
- **JIT**: los stores en línea (`SetPropR` propio, `SetElemR`,
  `SetLocalR`, `SetGlobalR` de propiedad del global) miran
  `heap.marking` y, si está activo, el byte de marca del objeto (offset
  de `marks` sondeado en `jit/layout.rs`); solo un objeto aún no
  escaneado va al helper (cuyo `get_mut` lo escanea).
- **Barrido perezoso** por pasos; libera objetos blancos y quita de las
  WeakMap/WeakSet vivas las entradas de claves muertas. Los huecos
  liberados se reutilizan solo al terminar el barrido (y entonces sube
  `collections`, la época de las cachés de llamada), así que un id no
  cambia de objeto dentro de un ciclo.
- **Ritmo**: el disparo es el de antes (objetos o bytes desde el último
  ciclo). Durante un ciclo, un paso de 1 ms (`RH_GC_SLICE_US`) cada 2048
  objetos o 512 KB asignados; si la asignación adelanta al colector, pasos
  más largos (hasta 8×) y, al cuádruple del presupuesto, el resto de
  golpe. Fuera de la ejecución de scripts, `Realm::gc_idle(deadline)`
  continúa el ciclo o empieza uno a mitad del umbral: lo llaman el bucle
  del hilo de contenido de la GUI (pasos de ≤ 4 ms, mensajes primero), el
  `settle` del CLI y `shot` cuando no hay nada pendiente.
  `RH_GC_INCREMENTAL=0` vuelve a las colecciones completas.
- **Estrés**: `RH_GC_STRESS=inc` hace 1–48 unidades de trabajo del
  colector en cada safepoint (tick 1), con ciclos encadenados; el modo
  completo (`RH_GC_STRESS=1`) ya no depende de la feature `jit`.

Validación: `jit-check` (js, script, frameworks; 354 tests) bajo
`RH_GC_STRESS=inc` con JIT apagado y a umbral 0: todo pasa e idéntico,
salvo `tests_webapi::indexeddb_in_memory` a umbral 0 en una de las dos
tandas — el fallo intermitente con carga de S2.6 (el equipo estaba a
carga 45; solo, con el mismo estrés, pasa siempre). Sin estrés, igual. `jitdiff` bajo
`RH_GC_STRESS=inc`: 30 000 programas + el corpus de `perf.rs` con el
binario anterior al barrier en línea y otros 30 000 con el final, 0
diferencias. Tests nuevos: barrera a
nivel de heap y un programa que mueve referencias (objetos, arrays,
closures, Map/Set, WeakMap, generadores, prototipos) mientras se marca.
`gc_memory` pasa (también bajo estrés).

Resultados (mismo binario, `RH_GC_INCREMENTAL=0` frente al incremental,
alternos, bajo `heavy.lock`):

| | antes: máx. / p95 | ahora: máx. / p95 | GC antes → ahora |
|---|---:|---:|---:|
| Speedometer (subconjunto) | 12,4–12,6 / 11,1–11,3 ms | 1,5 / 1,0 ms | 418–423 → 426–431 ms |
| YouTube | 46–55 / 45–48 ms | 3,5–3,6 / 1,0 ms | 236–271 → 169–195 ms en script (+53–80 ms en reposo) |
| diario grande | 26–32 / 25–30 ms | 1,6–1,8 / 1,0 ms | 381–457 → 291–343 ms |

(JIT apagado y encendido dan las mismas pausas; las cifras de cada fila
cubren ambos.)

- Speedometer no tiene tiempo ocioso en `shot` (siempre hay un
  temporizador vencido), así que todo el trabajo sigue dentro de la
  ventana medida, repartido en pasos de 1 ms. Geomean: paridad dentro
  del ruido (+0,1 … +1,6 % en dos tandas de 4–5 rondas; pasos de 2 y
  4 ms no cambian nada).
- Generación joven: no se hace. El marcado es ~1/3 del coste de la GC y
  la GC ~4–5 % del tiempo de Speedometer; una nursery ahorraría como
  mucho ~1,5 % a cambio de barrera permanente (no solo durante el
  marcado) y conjunto recordado. El barrido (liberar) no se ahorra.
- JIT por defecto (criterio de S2.7): `perf.rs` sigue sin casos
  < 0,95× (peor: array_map 0,96, push_objects 0,97; total 1,27×). En
  Speedometer, con la GC incremental en ambos lados, JIT/VM = +1,5 %
  (8 rondas), +0,5 % (5) y +0,7 % (6): Charts ya no se lleva la
  recolección entera (0,996–1,013×), pero TodoMVC y NewsSite siguen
  1–2 % por encima. **No se cumple «≥ 0 %»: el JIT sigue apagado**; lo
  que queda es del propio código nativo (llamadas, S2.7), no de la GC.
  Cambiar `ENTRY_MIN_OPS` no lo arregla (5 rondas: 24 → +1,1 %, 48 →
  +2,3 %, solo bucles → +1,0 %).

