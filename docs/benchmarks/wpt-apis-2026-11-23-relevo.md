# Relevo de WPT_API para revisión e integración

**Entrega del 23 de noviembre de 2026 a las 17:51 CET.** La rama local `WPT_API` parte de `a43c8e0`. El primer corte compartido con el revisor fue `8d6a0be9`. El último cambio de código está en `878adf0e95ccde72edc68ec848aab5e5974d6bff`. El SHA final, incluido este documento, figura en `/home/sergio/rabbithole-tools/WPT_API-LISTO.txt`.

## Tramos de la rama

| Tramo de Git | Commits | Evidencia | Prioridad |
|---|---:|---|---|
| `a43c8e0..367874b4` | 49 | Medición completa de las mismas 40.172 páginas en main y en la candidata. La rama pasó 1.467.543/1.776.087 (82,63 %), frente a 781.820/1.773.653 (44,08 %). Hay 701.434 ganancias y 15.711 pérdidas de subtests previamente aprobados. | Empezar por aquí. Revisar las pérdidas por identidad y las dependencias entre cambios. |
| `367874b4..8d6a0be9` | 97 | Contiene correcciones posteriores de DOM, edición, CSSOM, layout, WebCrypto, Fetch y WebRTC, con comparaciones dirigidas. Incluye la actualización de la baseline a la medición de `367874b4`. | Revisar por orden de commit o por dependencia. El porcentaje global de la punta queda por medir. |
| `8d6a0be9..878adf0e` | 11 | Edición, CSSOM, trayectorias CSS, descripciones ICE, fragmentos HTML, RSA y documentación. | Aplicar tras el corte anticipado. Los resultados dirigidos de este tramo figuran más abajo. |

Las ramas que ya existieran en el repositorio para otros trabajos quedan fuera de este relevo. Cada tramo incluye sus commits de documentación y baseline. El commit que cierra este informe viene después del último cambio de código. El anexo de la última medición completa detalla APIs por directorio. [Tabla WPT completa y pérdidas](wpt-apis-2026-11-23-367874b4.md)

## Cómo empezar en un árbol independiente

El checkout de desarrollo está en `/home/sergio/repos/rh-WPT_API`. En la misma máquina, el siguiente comando abre una copia de revisión en el último commit de la rama entregada. Si ya tienes un worktree propio, puedes inspeccionar los hashes desde él.

```sh
git -C /home/sergio/repos/rabbithole worktree add --detach /home/sergio/repos/rh-wpt-review-final WPT_API
cd /home/sergio/repos/rh-wpt-review-final
git log --reverse --format='%h %s' a43c8e0..367874b4
git log --reverse --format='%h %s' 367874b4..8d6a0be9
git log --reverse --format='%h %s' 8d6a0be9..HEAD
git diff --stat a43c8e0..HEAD
```

El checkout separado permite revisar e identificar bloques aptos para tu rama de integración. Para reproducir un resultado, conserva el SHA asociado a la medición. Si seleccionas commits individuales, comprueba sus dependencias y valida el árbol que resulte de combinarlos. Por ejemplo, `8d6a0be9` reutiliza la lista de puertos restringidos de `84bdd663`.

## Disponibilidad de TheOtherSide para rendimiento

**TheOtherSide quedó liberada por el autor de WPT_API a las 17:51 CET.** Terminaron las compilaciones y pruebas de esta rama, se conservaron los resultados y se eliminaron las copias temporales y los productos de compilación identificados como propios. Se conservaron las dependencias compartidas de Cargo. La comprobación final de procesos propios dio cero. Los directorios compartidos de otros trabajos siguen a cargo de sus propietarios. La señal acordada es `/home/sergio/rabbithole-tools/WPT_API-LISTO.txt`. Si se abre `WPT_API-ESPERA.txt`, vuelve a comprobar el estado antes de reservar rendimiento.

Antes del A/B comprueba la carga de los otros usuarios. El procedimiento prescribe carga de un minuto inferior a 2, dos builds alineados, ocho pares alternados y el CPU 8. Reserva `/tmp/rabbithole-compile-heavy.lock` durante las mediciones. Durante los builds, el guardián gestiona ese mismo bloqueo. Si coincide otro trabajo intensivo con un par, repítelo.

## Qué está medido

| Área | Resultado disponible | Alcance |
|---|---|---|
| APIs web globales | 44,08 % en main y 82,63 % en `367874b4` | WPT `831baf28d226911e47b2454d69e0c9e7348dd4d1`, 38.226 URL por el runner nativo y 1.946 variantes suplementarias. Una página sin subtests cuenta una vez por el estado del harness. `20e23c1f` guarda esa baseline. |
| Medición posterior parcial | 20.948 páginas cotejadas en 28 directorios. Main 268.754/469.990, `4a3abc07` 333.359/472.811. Hay 15.076 aprobados anteriores perdidos o ausentes en esta selección. | Comparación interrumpida al pasar al relevo. |
| Edición enriquecida | En 727 páginas, 107.898/116.349 → 108.946/116.354 subtests con nombre. 1.073 ganados, 25 perdidos. 76/76 regresiones del motor pasaron en la candidata de espacios. | `6eac591c`. Las 25 pérdidas afectan a la serialización de espacios ordinarios e indivisibles. |
| Fetch, WebSocket y WebRTC | Frente a main, 521/973 en Fetch (+12), 984/2.037 en WebSocket (+18) y 891/2.341 en WebRTC (+865). Cero pérdidas individuales en los tres grupos. Pasaron 2 pruebas nativas de red y 4 del transporte RTC. | 1.065 páginas en total, incluidos 61 casos de Fetch Request, 739 de WebSocket y 265 de WebRTC. Cambios en `84bdd663` y `8d6a0be9`. |
| CSS visual | Main 16.398/19.627, candidata anterior 16.435/19.627. 37 ganancias y cero pérdidas en esa ejecución. | Comparación por ruta en un punto anterior del desarrollo de anclajes. [Comparación visual por directorio](wpt-apis-2026-11-23-ranges-and-lists.md) |
| Unitarias, GC y JavaScript | En `d9db0456`, 1.657 unitarias aprobadas y 8 ignoradas. Con estrés incremental del GC pasaron 770 pruebas en cada modo. test262 dio 46.017/46.064 en ambos modos con el código JS/JIT de `eed3dd5a`, cero regresiones. | [Controles de la revisión medida](wpt-apis-2026-11-23-eed3dd5a.md) |
| Rendimiento | En `d9db0456`, ocho pares de Speedometer 3.1, mediana de 234,9 ms en main y 233,4 ms en la rama. | CPU 8, builds alineados. El guardián detuvo las compilaciones posteriores por el mínimo de espacio libre. El A/B de la punta queda abierto. |

## Resultado del último tramo

Estas ejecuciones usaron TheOtherSide, el WPT fijado, 10 procesos y 5/15 segundos de timeout por página en el runner nativo. Las fracciones cuentan subtests con nombre. El cotejo de ganancias y pérdidas también conserva la multiplicidad de nombres y el resultado de harness de las páginas vacías. Cada fila compara las mismas páginas entre sus dos extremos. Los resultados parciales se solapan con la medición global anterior.

| Área | Referencia | Páginas | Antes | Después | Ganados | Perdidos |
|---|---|---:|---:|---:|---:|---:|
| Edición | antes de los cambios de espacios | 727 | 107.898/116.349 | 108.961/116.354 | 1.063 | 0 |
| WebRTC | 4a3abc07 | 265 | 891/2.340 | 909/2.340 | 18 | 0 |
| WebCrypto | 4a3abc07 | 333 | 48.548/70.607 | 53.312/70.793 | 4.764 | 0 |
| css/cssom | 4a3abc07 | 272 | 3.659/4.458 | 3.717/4.458 | 58 | 0 |
| css/motion | 4a3abc07 | 46 | 143/4.959 | 3.556/4.959 | 3.425 | 12 |
| css/css-animations | 4a3abc07 | 129 | 800/1.431 | 797/1.431 | 2 | 5 |
| css/css-shapes | 4a3abc07 | 147 | 4.646/7.527 | 6.259/7.527 | 1.613 | 0 |
| css/css-transforms | 4a3abc07 | 108 | 4.019/5.516 | 4.019/5.516 | 0 | 0 |
| css/selectors | 4a3abc07 | 302 | 4.333/6.075 | 4.348/6.075 | 15 | 0 |
| custom-elements | 367874b4 | 200 | 2.933/4.205 | 3.022/4.200 | 89 | 0 |
| html/webappapis | 4a3abc07 | 163 | 121/371 | 133/371 | 12 | 0 |
| sanitizer-api | 367874b4 | 50 | 1.587/3.802 | 1.609/3.802 | 22 | 0 |
| shadow-dom | 367874b4 | 56 | 7.622/7.793 | 7.622/7.793 | 0 | 0 |

Pasaron las 78 regresiones del motor en `43b62f2e`, las cinco pruebas de transformaciones en la implementación de movimiento y el control de las cifras de `about:home`. Los cuatro controles de script cubrieron RSA, el filtrado y ciclo de vida de fragmentos HTML, la señalización RTC y las descripciones ICE. La última corrección de prioridad en el saneamiento HTML pasó también su prueba de regresión. Los manifiestos `handoff-source.sha256` y `handoff-filter-source.sha256` identifican los 888 archivos cotejados para las dos compilaciones finales.

Las 12 pérdidas de movimiento frente a `4a3abc07` corresponden a sintaxis válidas de `ray()` y `shape()` aún sin implementar por completo. Las cinco pérdidas de fotogramas involucran porcentajes expresados con `calc()`. En movimiento sigue habiendo 1.294 pérdidas individuales respecto a main. El resultado neto de las correcciones de espacios fue +1.063/−0 respecto a la revisión anterior a esa serie, con 196 pérdidas históricas respecto a main en el directorio de edición. Se conservan los resultados y las identidades de los fallos en `handoff-audit.json`, `handoff-filter-audit.json` y `close-02-audit.json`.

La comparación independiente con el servidor oficial y la CA de loopback cubrió las **1.482 páginas** de política de referrer. Main (`a43c8e0`) pasó 4.986/8.807 y la rama (`4a3abc07`) 4.992/8.807, con 246 ganancias y 240 pérdidas individuales. Ambos motores usaron el mismo adaptador TLS, los mismos puertos, ocho trabajadores y los límites de 5/15 segundos del manifiesto. Se comprobó que cada URL aparecía una vez por motor. En las variantes 4K+1, Fetch y XHR enviaron la ruta de carga original tras un cambio de URL con la API History. Las variantes alrededor del umbral de 4.096 bytes agrupan 240 pérdidas. El fichero `differences-by-fixture.json` desglosa las diferencias por grupo e incluye un ejemplo de cada resultado. Estos datos se guardan en `referrer-full-paired/`, separados de la baseline global recogida con la infraestructura anterior.

## Qué queda para decidir la integración

1. **Medir el árbol que se vaya a integrar.** Repetir los modos unitarios y de estrés GC, test262 normal y JIT forzado, los 19.627 reftests visuales, WPT API con el mismo manifiesto y Speedometer 3.1 A/B. La cifra histórica de 82,1 % del 13 de noviembre cubría solo 2.843 páginas. Las instrucciones de TheOtherSide y del guardián de disco están en `/var/www/html/apks/rabbithole-como-validar.md`, secciones 0–5.
2. **Revisar las regresiones por identidad.** En la última ejecución global, movimiento CSS agrupó 4.550 pérdidas, gaps 2.380, shapes 2.064, masking 1.496 y anclajes 730. Se solapan con 7.358 pérdidas que mencionan transiciones CSS. La repetición HTML con servidor nuevo recuperó 20 de los 99 resultados perdidos. [Edición, anclajes y repetición HTML](wpt-apis-2026-11-23-1fa7fca5.md)
3. **Comprobar las condiciones del servidor.** El runner integrado y el servidor oficial de WPT pueden producir resultados distintos en páginas que ejecutan Python o requieren HTTPS. En cuatro páginas de política de referrer, la primera comparación oficial con CA local válida dio 0/48 en main y 48/48 en la rama. Conservar ambos motores, el mismo servidor y las opciones de confianza al reproducirlas. [Comparación de servidores y TLS](wpt-apis-2026-11-23-b4d72643.md)
4. **Inspeccionar los cambios sensibles.** Priorizar coste e invalidación de layout, efectos de las animaciones en geometría, límites de edición, orden de eventos de WebSocket, generaciones ICE y vida útil de claves nativas. En el saneamiento HTML, revisar el orden entre exclusión de nodos prohibidos, conservación de hijos permitidos y activación de elementos personalizados. Las pruebas de datos WebRTC comprueban ICE, DTLS y SCTP locales. Captura de dispositivos, interoperabilidad de audio/vídeo y DRM comercial requieren infraestructura adicional.

Los JSONL, los informes del guardián y los cotejos están en `/home/sergio/rabbithole-tools/measurements/wpt-api-90`. Allí, `audit-367874b4/paired/api.tsv`, `gained.jsonl.gz` y `lost.jsonl.gz` (en el mismo subdirectorio) permiten pasar del resumen al subtest concreto. La referencia de main y el inventario oficial están en el directorio hermano `wpt-api-20261123-d9db`.
