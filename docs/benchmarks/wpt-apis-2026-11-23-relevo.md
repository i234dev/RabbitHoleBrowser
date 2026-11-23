# Relevo de WPT_API para revisión e integración

**Corte disponible el 23 de noviembre de 2026 a las 16:51 CET — `8d6a0be9`.** Hay una sola rama de desarrollo local, `WPT_API`, creada desde `a43c8e0`. Hasta este corte hay 146 commits, todos en una línea de historia. El revisor puede empezar por esos commits mientras el autor cierra el trabajo posterior.

## Qué puedes revisar ya

| Tramo de Git | Commits | Evidencia | Prioridad |
|---|---:|---|---|
| `a43c8e0..367874b4` | 49 | Medición completa de las mismas 40.172 páginas en main y en la candidata. La rama pasó 1.467.543/1.776.087 (82,63 %), frente a 781.820/1.773.653 (44,08 %). Hay 701.434 ganancias y 15.711 pérdidas de subtests previamente aprobados. | Empezar por aquí. Revisar las pérdidas por identidad y las dependencias entre cambios. |
| `367874b4..8d6a0be9` | 97 | Contiene correcciones posteriores de DOM, edición, CSSOM, layout, WebCrypto, Fetch y WebRTC, con comparaciones dirigidas. Incluye la actualización de la baseline a la medición de `367874b4`. | Revisar por orden de commit o por dependencia. El porcentaje global de la punta queda por medir. |
| Después de `8d6a0be9` | En curso al escribir este relevo | El autor está cerrando los cambios de espacios en edición, selectores y fotogramas de CSSOM, trayectorias CSS, descripciones ICE, fragmentos HTML y RSA. | El autor comunicará el hash final. El intervalo restante será `8d6a0be9..<hash-final>`. |

Las ramas que ya existieran en el repositorio para otros trabajos quedan fuera de este relevo. Los commits de documentación y de baseline también cuentan en los totales anteriores. El anexo de la última medición completa detalla APIs por directorio. [Tabla WPT completa y pérdidas](wpt-apis-2026-11-23-367874b4.md)

## Cómo empezar en un árbol independiente

El checkout de desarrollo está en `/home/sergio/repos/rh-WPT_API`. En la misma máquina, el siguiente comando abre una copia de revisión en el hash congelado. Si ya tienes un worktree propio, puedes inspeccionar los hashes desde él.

```sh
git -C /home/sergio/repos/rabbithole worktree add --detach /home/sergio/repos/rh-wpt-review-8d6a0be9 8d6a0be9
cd /home/sergio/repos/rh-wpt-review-8d6a0be9
git log --reverse --format='%h %s' a43c8e0..367874b4
git log --reverse --format='%h %s' 367874b4..8d6a0be9
git diff --stat a43c8e0..8d6a0be9
```

El checkout separado permite revisar e identificar bloques aptos para tu rama de integración. El autor seguirá añadiendo commits posteriores en `WPT_API`. Para reproducir un resultado, conserva el SHA asociado a la medición. Si seleccionas commits individuales, comprueba sus dependencias y valida el árbol que resulte de combinarlos. Por ejemplo, `8d6a0be9` reutiliza la lista de puertos restringidos de `84bdd663`.

## Disponibilidad de TheOtherSide para rendimiento

**A las 16:53 CET del 23 de noviembre, el autor de WPT_API está usando TheOtherSide para compilaciones y pruebas dirigidas. La ventana de rendimiento sigue pendiente.** En ese momento se estaba ejecutando WPT de edición y la carga media de un minuto era 2,90. Quedan comprobaciones y posibles recompilaciones. Se puede empezar ya la revisión del código en un worktree propio.

Los trabajos del autor usan los CPU 0–3 y 10–15. Aunque el CPU 8 esté libre, una compilación o un barrido WPT paralelo compite por memoria, caché, disco y presupuesto térmico. Una pausa entre lotes puede parecer una máquina libre durante unos segundos.

Para el Speedometer 3.1 A/B, **espera al aviso explícito de liberación de TheOtherSide por el autor de WPT_API**. Después, comprueba también la actividad de otros usuarios y que la carga de un minuto sea inferior a 2. Puedes consultar la máquina sin iniciar una compilación:

```sh
ssh -p 2222 -i ~/.ssh/rabbithole_wsl rhbuild@192.168.1.101 \
  'date -Is; uptime; ps -eo pid,comm,pcpu --sort=-pcpu | head -15; lslocks -o PID,COMMAND,PATH | grep -F rabbithole-compile-heavy.lock'
```

Con la ventana disponible, usa dos builds alineados, ocho pares alternados y el CPU 8. Reserva el bloqueo compartido `/tmp/rabbithole-compile-heavy.lock` durante las mediciones para impedir que se cuele otra compilación gestionada por el guardián. Las compilaciones previas adquieren ese bloqueo a través del propio guardián. Si una compilación u otro trabajo pesado coincide con un par, repítelo antes de usarlo para decidir la integración.

## Qué está medido

| Área | Resultado disponible | Alcance |
|---|---|---|
| APIs web globales | 44,08 % en main y 82,63 % en `367874b4` | WPT `831baf28d226911e47b2454d69e0c9e7348dd4d1`, 38.226 URL por el runner nativo y 1.946 variantes suplementarias. Una página sin subtests cuenta una vez por el estado del harness. `20e23c1f` guarda esa baseline. |
| Medición posterior parcial | 20.948 páginas cotejadas en 28 directorios. Main 268.754/469.990, `4a3abc07` 333.359/472.811. Hay 15.076 aprobados anteriores perdidos o ausentes en esta selección. | Comparación interrumpida al pasar al relevo. |
| Edición enriquecida | En 727 páginas, 107.898/116.349 → 108.946/116.354 subtests con nombre. 1.073 ganados, 25 perdidos. 76/76 regresiones del motor pasaron en la candidata de espacios. | `6eac591c`. Las 25 pérdidas afectan a la serialización de espacios ordinarios e indivisibles. |
| Fetch, WebSocket y WebRTC | Frente a main, 521/973 en Fetch (+12), 984/2.037 en WebSocket (+18) y 891/2.341 en WebRTC (+865). Cero pérdidas individuales en los tres grupos. Pasaron 2 pruebas nativas de red y 4 del transporte RTC. | 1.065 páginas en total, incluidos 61 casos de Fetch Request, 739 de WebSocket y 265 de WebRTC. Cambios en `84bdd663` y `8d6a0be9`. |
| CSS visual | Main 16.398/19.627, candidata anterior 16.435/19.627. 37 ganancias y cero pérdidas en esa ejecución. | Comparación por ruta en un punto anterior del desarrollo de anclajes. [Resultados y condiciones](wpt-apis-2026-11-23-1fa7fca5.md) |
| Unitarias, GC y JavaScript | En `d9db0456`, 1.657 unitarias aprobadas y 8 ignoradas. Con estrés incremental del GC pasaron 770 pruebas en cada modo. test262 dio 46.017/46.064 en ambos modos con el código JS/JIT de `eed3dd5a`, cero regresiones. | [Controles de la revisión medida](wpt-apis-2026-11-23-eed3dd5a.md) |
| Rendimiento | En `d9db0456`, ocho pares de Speedometer 3.1, mediana de 234,9 ms en main y 233,4 ms en la rama. | CPU 8, builds alineados. El guardián detuvo las compilaciones posteriores por el mínimo de espacio libre. El A/B de la punta queda abierto. |

## Qué queda para decidir la integración

1. **Medir el árbol que se vaya a integrar.** Repetir los modos unitarios y de estrés GC, test262 normal y JIT forzado, los 19.627 reftests visuales, WPT API con el mismo manifiesto y Speedometer 3.1 A/B. La cifra histórica de 82,1 % del 13 de noviembre cubría solo 2.843 páginas. Las instrucciones de TheOtherSide y del guardián de disco están en `/var/www/html/apks/rabbithole-como-validar.md`, secciones 0–5.
2. **Revisar las regresiones por identidad.** En la última ejecución global, movimiento CSS agrupó 4.550 pérdidas, gaps 2.380, shapes 2.064, masking 1.496 y anclajes 730. Se solapan con 7.358 pérdidas que mencionan transiciones CSS. El último resultado de edición por sí solo conserva 25 pérdidas frente al paso previo a los cambios de espacios. La repetición HTML con servidor nuevo recuperó 20 de los 99 resultados perdidos. [Edición, anclajes y repetición HTML](wpt-apis-2026-11-23-1fa7fca5.md)
3. **Comprobar las condiciones del servidor.** El runner integrado y el servidor oficial de WPT pueden producir resultados distintos en páginas que ejecutan Python o requieren HTTPS. En cuatro páginas concretas de política de referrer el control oficial con CA local válida dio 0/48 en main y 48/48 en la rama. Conservar ambos motores, el mismo servidor y las opciones de confianza al reproducirlas. [Comparación de servidores y TLS](wpt-apis-2026-11-23-b4d72643.md)
4. **Inspeccionar los cambios sensibles.** Priorizar coste e invalidación de layout, efectos de las animaciones en geometría, límites de edición, orden de eventos de WebSocket, generaciones ICE y vida útil de claves nativas. Las pruebas de datos WebRTC comprueban ICE, DTLS y SCTP locales. Captura de dispositivos, interoperabilidad de audio/vídeo y DRM comercial requieren infraestructura adicional.

Los JSONL, los informes del guardián y los cotejos están en `/home/sergio/rabbithole-tools/measurements/wpt-api-90`. Allí, `audit-367874b4/paired/api.tsv`, `gained.jsonl.gz` y `lost.jsonl.gz` (en el mismo subdirectorio) permiten pasar del resumen al subtest concreto. La referencia de main y el inventario oficial están en el directorio hermano `wpt-api-20261123-d9db`.
