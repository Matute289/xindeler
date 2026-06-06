# Terrain Resolution Improvement — Design Spec

> **For agentic workers:** Este documento es el spec completo del proyecto. Usá `superpowers:writing-plans` para crear el plan de implementación de cada fase. Implementar una fase a la vez en orden: Fase 1 → Fase 2 → Fase 3.

**Objetivo:** Mejorar la calidad visual y física del terreno de Veloren en tres fases incrementales, cada una shippeable de forma independiente.

**Enfoque elegido:** Combinado en fases — primero rendering suavizado (resultado rápido), luego bloques más pequeños (detail físico), luego micro-detalle visual.

**Estilo visual objetivo:** Soft voxel — bordes biselados entre bloques usando el algoritmo Transvoxel. Los bloques todavía se reconocen pero las transiciones son suaves. Colisión coincide con la superficie visual.

**Configurabilidad:** Cada mejora se expone como opción en Settings → Graphics para que el usuario elija según su hardware.

---

## Estado actual del engine (contexto)

- **Chunk size:** 32×32 bloques horizontales (`TERRAIN_CHUNK_BLOCKS_LG = 5` en `common/src/terrain/mod.rs:46`)
- **Block world-space size:** ~0.3m por bloque (implícito en física y world gen, no hay una constante central)
- **Mesher actual:** Greedy meshing en `voxygen/src/mesh/terrain.rs` — genera quads por cara visible, sin suavizado
- **Colisión actual:** AABB por bloque en `common-systems/src/phys.rs` — puramente rectangular
- **World gen:** Procedural en `world/src/` — todas las alturas y distancias hardcodeadas en unidades de bloque
- **Networking:** Chunks serializados con formato fijo cliente↔servidor
- **Presets de calidad existentes:** `minimal/low/medium/high/ultra` en `voxygen/src/settings/graphics.rs`
- **Auto-detect de GPU:** Implementado (primer launch detecta GPU y aplica preset automático)

---

## Fase 1 — Soft Voxel Rendering + Colisión Suavizada

**Duración estimada:** 2–3 meses  
**Riesgo:** Medio (nuevo subsistema de meshing + colisión, sin tocar world gen ni networking)  
**Resultado:** El terreno se ve suavizado, los bordes entre bloques se biselan, y el personaje camina sobre la curva real — no sobre bloques rectangulares invisibles.

### Cómo funciona el Transvoxel

El algoritmo de Eric Lengyel (2010) trabaja sobre un **campo de densidad** generado a partir de los bloques existentes:

1. Cada bloque sólido = densidad 255, cada bloque vacío = densidad 0
2. Se aplica un kernel de suavizado 3×3×3 para crear transiciones graduales en los bordes
3. El algoritmo marcha por grupos de 2×2×2 bloques y genera triángulos interpolados en los bordes
4. El resultado es una superficie de triángulos que sigue la forma del terreno pero sin bordes rectos

**Importante:** el world gen, el servidor, el formato de chunk y los datos de bloque no cambian. Transvoxel es una capa de interpretación encima de los datos existentes.

### Nuevos archivos

```
common/src/terrain/density.rs
    - Función: convert_chunk_to_density_field(chunk: &TerrainChunk) -> DensityField
    - Función: smooth_density_field(field: &mut DensityField, kernel_size: u8)
    - Struct: DensityField { data: Vec<u8>, size: Vec3<u32> }
    - Compartido entre cliente (rendering) y servidor (colisión)

voxygen/src/mesh/transvoxel.rs
    - Función: mesh_transvoxel(density: &DensityField, lod: u8) -> (Mesh<TerrainVertex>, Mesh<FluidVertex>)
    - Implementación completa del algoritmo Transvoxel de Lengyel
    - Maneja 3 niveles de LOD: distancia corta (full), media (reducido), larga (muy reducido)

common-systems/src/phys_smooth.rs
    - Función: extract_collision_triangles(density: &DensityField) -> Vec<Triangle>
    - Integración con el sistema de física existente para colisión por triángulos
    - Solo activo cuando TerrainSmoothingMode != Disabled

voxygen/src/settings/graphics.rs (modificar)
    - Agregar enum TerrainSmoothingMode { Disabled, Soft, Smooth, Ultra }
    - Agregar campo terrain_smoothing: TerrainSmoothingMode a GraphicsSettings
    - Disabled → mesher actual (greedy), Soft/Smooth/Ultra → transvoxel con distintos LOD
```

### Archivos modificados

```
voxygen/src/mesh/terrain.rs
    - Switch entre GreedyMesh y Transvoxel según terrain_smoothing setting

common-systems/src/phys.rs
    - Integrar colisión de triángulos cuando phys_smooth está activo
    - Fallback a AABB si TerrainSmoothingMode::Disabled

voxygen/src/settings/graphics.rs
    - Agregar TerrainSmoothingMode a los presets into_low/medium/high/ultra:
      low → Disabled, medium → Soft, high → Smooth, ultra → Ultra
    - Agregar al auto_detect() según GPU tier

voxygen/src/scene/terrain.rs
    - Pasar density field al nuevo mesher
    - Cache del density field por chunk (no recalcular en cada frame)
```

### Niveles de calidad

| Nivel | LOD levels | Colisión suavizada | Normal maps | Hardware mínimo |
|---|---|---|---|---|
| Disabled | — | No (AABB) | No | Cualquiera |
| Soft | 1 | Sí | No | GTX 1060 / RX 580 |
| Smooth | 3 | Sí | No | RTX 3060 / RX 6600 |
| Ultra | 3 | Sí | Sí | RTX 3070+ / RX 6800+ |

### Integración con auto-detect

El `auto_detect()` en `voxygen/src/settings/graphics.rs` ya asigna presets por GPU. Extenderlo para incluir `terrain_smoothing`:
- GPU integrada / tier Low → `Disabled`
- GPU mid-range (GTX 16xx, RX 5xxx) → `Soft`
- GPU high-end (RTX 30xx, RX 6xxx) → `Smooth`
- GPU flagship (RTX 40xx, RX 7xxx) → `Ultra`

### Testing de Fase 1

1. Verificar que `Disabled` produce output idéntico al mesher actual
2. Verificar que con `Soft`/`Smooth` no hay gaps entre chunks (seams)
3. Verificar que el personaje no cae a través del terreno con colisión suavizada
4. Verificar que la colisión coincide visualmente con la superficie (sin "flotar")
5. Benchmark de FPS en GTX 1060 con `Soft` vs baseline

---

## Fase 2 — Bloques más pequeños (escala 0.3m → 0.15m)

**Duración estimada:** 4–6 meses  
**Riesgo:** Alto (cambio que se propaga por todo el engine)  
**Prerequisito:** Fase 1 completa y estable  
**Resultado:** El detalle físico del terreno se duplica — cuevas, costas y pendientes tienen el doble de fidelidad. El personaje es proporcionalmente más grande en bloques, lo que hace que los terrenos se vean más naturales.

### El cambio central

Reducir el tamaño world-space de cada bloque de 0.3m a 0.15m. En consecuencia:
- Un personaje de 1.8m pasa de ~6 bloques de alto a ~12 bloques
- El mundo necesita el doble de bloques en altura para mantener las mismas montañas/valles
- Los chunks cubren 4.8m×4.8m en vez de 9.6m×9.6m → más chunks para la misma distancia de visión

### Estrategia de migración (feature flag)

Esta fase se implementa detrás de un feature flag `terrain-hires` para poder desarrollar y testear sin romper el juego en producción:

```toml
# Cargo.toml
[features]
terrain-hires = []  # doble resolución de bloque
```

La migración es sistema por sistema:
1. World gen primero (con flag desactivado, el juego sigue funcionando con el viejo sistema)
2. Física y networking después
3. Activar flag cuando todos los sistemas están listos

### Sistemas afectados

**World generation (`world/src/`):**
- Todas las constantes de altura, distancia y densidad hardcodeadas deben multiplicarse por 2
- Ejemplo: si una montaña generaba hasta 300 bloques de alto (90m), debe generar hasta 600 (sigue siendo 90m)
- Archivos clave: `world/src/sim/`, `world/src/layer/`, `world/src/site/`
- Estrategia: buscar todos los literales numéricos relativos a coordenadas de bloque y auditarlos

**Networking (`common-net/`, `server/`, `client/`):**
- Los chunks siguen siendo 32×32 bloques (el formato no cambia)
- Pero ahora representan la mitad del área → el cliente necesita cargar más chunks para la misma view distance
- Ajustar `terrain_view_distance` en los presets para compensar (duplicar valores)

**Física (`common-systems/src/phys.rs`):**
- Velocidades, gravedad, radio de entidades y alturas de salto en unidades de bloque → dividir por 2
- Si la Fase 1 ya implementó colisión por triángulos, esta fase es más sencilla (los triángulos ya se adaptan)

**Modelos y entidades:**
- Los `.vox` no cambian pero su `scale` en los manifests `.ron` puede necesitar ajuste
- Las hitboxes de entidades están en `common/src/comp/body/` → todas en unidades de bloque → ajustar

**Saves y persistencia:**
- Las coordenadas guardadas en `userdata/` están en unidades de bloque
- Necesita migración de saves o versioning del formato

### Testing de Fase 2

1. Verificar que el mundo generado con el nuevo scale se ve proporcionalmente igual al actual
2. Verificar que el personaje no tiene velocidades o saltos incorrectos
3. Verificar que los sites (ciudades, dungeons) generan correctamente en la nueva escala
4. Verificar que los saves existentes no corrompen el mundo

---

## Fase 3 — Normal Maps + Micro-detalle

**Duración estimada:** 1–2 meses  
**Riesgo:** Bajo (cambios de shader y assets, sin gameplay)  
**Prerequisito:** Fase 1 completa (los normal maps se aplican sobre la geometría suavizada)  
**Resultado:** Cada tipo de bloque tiene textura superficial propia — la roca parece tallada, la tierra tiene granos, la nieve tiene cristales. Sin cambio en geometría real.

### Implementación

**Normal map atlas:**
```
assets/voxygen/texture/terrain_normals/
    grass.png        ← normal map para hierba
    rock.png         ← normal map para roca
    sand.png         ← normal map para arena
    snow.png         ← normal map para nieve
    dirt.png         ← normal map para tierra
    ...
```

Cada bloque en `common/src/terrain/block.rs` necesita un índice al normal map correspondiente.

**Shaders:**
```
voxygen/src/render/shaders/terrain.frag
    - Samplear el normal map según tipo de bloque
    - Combinar con la normal geométrica de Transvoxel
    - Parallax mapping para micro-desplazamiento a distancias cortas (solo Ultra)
```

**Settings:**
- Los normal maps son parte de `TerrainSmoothingMode::Ultra`
- El parallax mapping es parte del mismo tier
- No necesitan setting propio — reutilizan el tier Ultra de Fase 1

### Testing de Fase 3

1. Verificar que los normal maps no crean artifacts en los bordes de chunk
2. Verificar que el parallax mapping no causa Z-fighting
3. Verificar FPS con Ultra en hardware target (RTX 3070)

---

## Orden de implementación recomendado

```
Fase 1:
  1. common/src/terrain/density.rs              ← base de todo
  2. voxygen/src/mesh/transvoxel.rs             ← mesher visual
  3. voxygen/src/settings/graphics.rs           ← TerrainSmoothingMode
  4. voxygen/src/mesh/terrain.rs                ← switch greedy↔transvoxel  
  5. voxygen/src/scene/terrain.rs               ← integración en el pipeline
  6. common-systems/src/phys_smooth.rs          ← colisión de triángulos
  7. common-systems/src/phys.rs                 ← integrar colisión suavizada

Fase 2 (después de Fase 1 estable):
  1. Feature flag terrain-hires en Cargo.toml
  2. world/src/ — rescalar world gen
  3. common/src/comp/body/ — rescalar hitboxes
  4. common-systems/src/phys.rs — rescalar física
  5. server/ y client/ — ajustar view distance defaults
  6. Migración de saves

Fase 3 (puede hacerse en paralelo a Fase 2):
  1. assets/voxygen/texture/terrain_normals/ — crear normal maps
  2. voxygen/src/render/shaders/terrain.frag — integrar en shader
  3. common/src/terrain/block.rs — índice de normal map por tipo de bloque
```

---

## Decisiones de diseño y razonamiento

| Decisión | Alternativa descartada | Razón |
|---|---|---|
| Transvoxel para suavizado | Marching cubes puro | Transvoxel preserva identidad voxel, MC la elimina completamente |
| Colisión coincide con visual | Dejar AABB blocky | El usuario lo requirió explícitamente |
| Feature flag para Fase 2 | Migración directa | Alto riesgo; el flag permite desarrollo incremental sin romper el juego |
| Normal maps en Fase 3 separada | Normal maps en Fase 1 | Fase 1 ya es compleja; normal maps son independientes y de bajo riesgo |
| Settings por tier | Setting granular | Consistente con el sistema existente de presets |

---

---

## Fase 1 — Continuación: SmoothTerrainVertex Pipeline

**Prerequisito:** Las tareas anteriores de Fase 1 están completas (Transvoxel + atlas de color básico).  
**Problema a resolver:** `TerrainVertex` tiene posiciones enteras (6-bit x/y) y normales de 3 bits (6 direcciones axiales). Los vértices Transvoxel interpolados se truncan al grid entero y los normales suaves se redondean → shading plano visible (facetado).

### Decisión de diseño: pipeline separado (Opción B)

Se crea un `SmoothTerrainPipeline` completamente independiente del greedy pipeline existente. Razones:
- No contamina `TerrainVertex` con campos que el greedy mesher nunca usaría
- El smooth pipeline puede evolucionar (Fase 3: normal maps, LOD) sin afectar el greedy
- Separación limpia de concerns: dos meshers → dos pipelines → dos shaders

### Nuevo vertex format: `SmoothTerrainVertex`

```rust
// voxygen/src/render/pipelines/smooth_terrain.rs
#[repr(C)]
#[derive(Copy, Clone, Zeroable, Pod)]
pub struct SmoothTerrainVertex {
    pos:      [f32; 3],   // posición float en chunk-local coords (12 bytes)
    norm:     u32,        // normal packed 10-10-10-2 snorm (4 bytes)
    col_light: u32,       // RGBA color bakeado + light info (4 bytes)
}
// Total: 20 bytes/vértice (vs 8 bytes del TerrainVertex actual)
```

**Codificación del normal (10-10-10-2 snorm):**
- x: bits 0-9   → float en –1..1 mapeado a –511..511
- y: bits 10-19 → ídem
- z: bits 20-29 → ídem
- w: bits 30-31 → no usado (siempre 0)

**Codificación del color (`col_light`):**  
Mismo formato que `TerrainVertex::make_col_light` — compatible con el fragment shader de terreno existente para reutilizar la lógica de iluminación.

### Archivos a crear

```
voxygen/src/render/pipelines/smooth_terrain.rs
    - struct SmoothTerrainVertex
    - impl SmoothTerrainVertex::new(pos, norm, col_light)
    - fn pack_norm_10_10_10_2(norm: Vec3<f32>) -> u32
    - struct SmoothTerrainPipeline
    - impl SmoothTerrainPipeline::new(...)
    - SmoothTerrainPipeline::draw() binding

assets/voxygen/shaders/smooth-terrain-vert.glsl
    - Lee pos como vec3 float (location 0)
    - Lee norm como uint (location 1), decodifica a vec3
    - Lee col_light como uint (location 2)
    - Output: f_pos, f_norm, f_col_light (mismo layout que terrain-frag espera)

assets/voxygen/shaders/smooth-terrain-frag.glsl
    - Reutiliza #include <globals.glsl>, <srgb.glsl>, <lod.glsl>, <shadows.glsl>
    - Recibe f_norm como vec3 float (en lugar de decodificar desde pos_norm)
    - El resto del pipeline de iluminación es idéntico al terrain-frag.glsl existente
```

### Archivos a modificar

```
voxygen/src/render/pipelines/mod.rs
    - pub mod smooth_terrain;
    - Exportar SmoothTerrainVertex, SmoothTerrainPipeline

voxygen/src/render/mod.rs
    - Re-exportar SmoothTerrainVertex, SmoothTerrainPipeline
    - Agregar smooth-terrain-vert/frag a la lista de shaders compilados al startup

voxygen/src/render/renderer/pipeline_creation.rs
    - Crear SmoothTerrainPipeline junto al resto de pipelines

voxygen/src/scene/terrain/mod.rs
    - Agregar a TerrainChunkData:
        smooth_opaque_model: Option<Model<SmoothTerrainVertex>>
    - En el loop de render: si terrain_smoothing != Disabled, draw smooth_opaque_model
      con SmoothTerrainPipeline; si Disabled, draw opaque_model con TerrainPipeline

voxygen/src/mesh/terrain.rs
    - El path Transvoxel ya retorna early; cambiar el tipo de output de
      Mesh<TerrainVertex> a Mesh<SmoothTerrainVertex>
    - Construir SmoothTerrainVertex con pos float real (sin truncar) y
      normal 10-10-10-2 (sin cuantizar a 6 ejes)
```

### Flujo de datos

```
DensityField → mesh_transvoxel() → Vec<TransvoxelTriangle>
    ↓ (por cada vértice)
    pos float (field-local) + mesh_delta → SmoothTerrainVertex.pos
    density_gradient() → pack_norm_10_10_10_2() → SmoothTerrainVertex.norm
    atlas color lookup → make_col_light() → SmoothTerrainVertex.col_light
    ↓
Mesh<SmoothTerrainVertex> → GPU via SmoothTerrainPipeline
    ↓
smooth-terrain-vert.glsl → smooth-terrain-frag.glsl → frame buffer
```

### Testing

1. Verificar que `Disabled` no crea ningún `smooth_opaque_model` (sin regresión al greedy)
2. Verificar que con `Smooth` la superficie se ve sin facetado (comparar screenshot)
3. `cargo ci-clippy -- -D warnings` limpio
4. `cargo ci-clippy2 -- -D warnings` limpio (publish profile)

---

## Seguimiento de progreso

| Fase | Estado | Notas |
|---|---|---|
| Fase 1 — Transvoxel + colisión | ✅ Completa | Pipeline, shaders, física, threshold calibrado, triángulos, normales — funcional |
| Fase 2 — Escala de bloques | 🔄 En progreso | Task 7 completo; save migration pendiente (próximo paso) |
| Fase 3 — Normal maps | ⬜ No iniciada | Arrancar después de completar Fase 2 |

Actualizar esta tabla a medida que avanza la implementación:
- ⬜ No iniciada
- 🔄 En progreso
- ✅ Completa
- ⏸ Pausada

---

## Estado detallado al 2026-06-06

### Fase 1 — Completa ✅

Todo el pipeline Transvoxel está funcionando en el juego:
- `SmoothTerrainPipeline` + shaders `smooth-terrain-vert/frag.glsl`
- `TerrainSmoothingMode` enum (Disabled/Soft/Smooth/Ultra) en settings
- `DensityField` + `smooth_density_field` (Gaussian, N passes)
- `mesh_transvoxel` con threshold calibrado por passes (Soft=64, Smooth=94, Ultra=101)
- `density_gradient` con interpolación trilineal (paredes visibles)
- `col_light_for` con interpolación bilineal (sin patrón de diamantes)
- `has_structures` fallback greedy (chunks con boi.interactables/smokers/one_way_walls)
- Smooth floor physics correction (SmoothTerrainSettings resource)
- `convert_chunk_to_density_field`: Err→255 (sin triángulos transparentes por vecinos no cargados)
- GitHub CI limpio (8 workflows upstream eliminados, upstream-sync.yml YAML fixeado)

**Limitaciones conocidas (cosmética, para detallar post-Fase 3):**
- Paredes de edificios sin boi (plain walls) todavía algo invisibles
- Transición abrupta entre chunks smooth y greedy en bordes
- Terreno flat tiene leve textura triangular (cosmético)

### Fase 2 — En progreso 🔄

**HEAD terrain-hires:** `0dd4c91a2a` (branch: main)

**Commits completados (Task 1-5 del plan):**
- `59e878f257` — Feature flag + BLOCK_SIZE/HIRES_SCALE en common/Cargo.toml + consts.rs
- `0630fa36c0` — GRAVITY y MOVEMENT_THRESHOLD_VEL × HIRES_SCALE
- `6aab42dec3` — Humanoid collider height/width × HIRES_SCALE
- `994d2b8cb0` — World gen sea_level + mountain_scale × HIRES_SCALE
- `18fb5c97b2` — terrain_view_distance todos los presets × HIRES_SCALE

**Cómo activar y probar:**
```bash
source "$HOME/.cargo/env"
cargo run --bin veloren-voxygen --features veloren-voxygen/terrain-hires
```

**Task 7 — Estado final (2026-06-05):**
- ✅ `base_accel()` todas las especies × HIRES_SCALE (commit `0d7c2a4b64`)
- ✅ server `max_view_distance` × HIRES_SCALE + server/Cargo.toml feature (commit `075ed503be`)
- ✅ World gen: cave.rs, scatter.rs, column.rs, airship_travel.rs × HIRES_SCALE (commits `c74b4f3fee` + `9881ad3558`)
- ✅ Interaction ranges: MAX_PICKUP_RANGE, MAX_MOUNT_RANGE, etc. × HIRES_SCALE (commit `9881ad3558`)
- ⏪ `dimensions()` y `humanoid.height()` — **REVERTIDOS** (commit `0dd4c91a2a`). Ver nota crítica abajo.
- ❌ Save migration: coordenadas de bloque en `userdata/` necesitan ×2 al cargar con terrain-hires (pendiente — plan separado)

**⚠️ Nota crítica — Por qué `dimensions()` y `humanoid.height()` NO deben escalarse con HIRES_SCALE:**

Al escalarlos, los personajes y NPCs se veían 2× más grandes que el terreno. La causa raíz:

`dimensions()` en `common/src/comp/body/mod.rs` controla **simultáneamente** el hitbox físico Y la escala visual del modelo 3D. En Veloren, los hitboxes se definen en **unidades de bloque relativas al terreno** — un personaje humanoide mide ~2.6 bloques de alto, y una puerta estándar mide 3 bloques de alto. Esta relación proporcional debe mantenerse igual independientemente del tamaño world-space de un bloque.

Lo que sí debe escalar con HIRES_SCALE: constantes en unidades absolutas (m/s², m/s, metros) como GRAVITY, base_accel, rangos de interacción.  
Lo que NO debe escalar: proporciones entidad/terreno (bloques de alto de un personaje, radio de colisión en bloques).

`humanoid.height()` en `common/src/comp/body/humanoid.rs` también controla la escala visual del modelo — mismo principio.

**Plan completo:** `docs/superpowers/plans/2026-06-05-fase2-block-scale.md`

**Sistema de telemetría — ✅ Operativo (desde 2026-06-06)**

El sistema de logging está completamente implementado y activo. Al probar el juego con terrain-hires:
- `telemetry!` macro emite eventos JSON Lines en `userdata/voxygen/logs/client_telemetry.jsonl`
- Eventos cubiertos: session start/end, chunk load/unload, FPS/frame_ms, network connect, UI interactions
- `TelemetrySystem` ECS snapshots cada 150 ticks: world count, player stats, entity count
- Bug report activo: `https://veloren.greenmountain.dev/bug-report` — botón "Report Bug" en EscMenu envía los últimos 500 líneas de telemetría + 200 de error log al VPS

Para probar con telemetría activa (logging-verbose escribe el JSON Lines):
```bash
cargo run --bin veloren-voxygen \
  --features "veloren-voxygen/terrain-hires,veloren-voxygen/logging-verbose"
```

Esto permite observar en tiempo real: FPS con terrain-hires activo, chunks que tardan en cargar, crashes/errores.

**Próximo paso — Save migration:**

### Pruebas visuales — 2026-06-06 (Fase 1 + Fase 2 parcial)

Screenshots en `/Users/mgrinberg/MyVeloren/Screenshots/smooth-enabled-{1,2,3}.png`.

**Observaciones:**
- ✅ Transvoxel rendering funciona — los facets triangulares son claramente visibles en terreno abierto (screenshot 3)
- ✅ Terreno de pueblo/villa renderiza con suavizado (screenshots 1 y 2)
- ✅ Geometría smooth en estructuras: el edificio detrás del personaje en screenshot 1 muestra las caras triangulares características
- ✅ Suelo plano con ligera textura triangular (limitación conocida, cosmética)
- ⚠️ Screenshots tomados con build anterior al botón "Report Bug" — el EscMenu tiene 6 botones (el actual tiene 7)
- ℹ️ Área de agua en screenshot 3 muestra el efecto Transvoxel más pronunciado — las caras anguladas del fluido son llamativas; puede ser candidato a ajuste visual en Fase 3

### Fase 3 — No iniciada ⬜

Arrancar después de completar save migration (Fase 2).
Ver sección Fase 3 del spec arriba.

---

## Prompt de reanudación

```
Lee docs/superpowers/specs/2026-06-04-terrain-resolution-design.md, sección "Estado detallado al 2026-06-06".
Luego: git log --oneline -8 && git status

Estado actual:
- Fase 1: completa ✅
- Fase 2: en progreso 🔄 (HEAD: 9881ad3558)
  - Task 7 completo. dimensions()/humanoid.height() NO escalar (ver nota crítica en spec).
  - Pendiente: save migration — coordenadas de bloque en userdata/ necesitan ×2 al cargar con terrain-hires.
- Fase 3: no iniciada ⬜ — arrancar después de save migration
- Sistema de telemetría: ✅ operativo. Para probar con telemetría activa:
    cargo run --bin veloren-voxygen \
      --features "veloren-voxygen/terrain-hires,veloren-voxygen/logging-verbose"
  Logs en: userdata/voxygen/logs/client_telemetry.jsonl
  Bug report VPS: https://veloren.greenmountain.dev/bug-report

Próximo paso concreto:
→ Implementar save migration (plan separado a crear con writing-plans).
  Leer primero: docs/superpowers/plans/2026-06-05-fase2-block-scale.md para ver qué falta.

No hagas preguntas — toda la info está en la spec y en los planes linkados.
```
