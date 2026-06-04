# Superpowers Infrastructure — Veloren

**Date:** 2026-06-03
**Status:** Approved
**Scope:** Project-level skills, hooks y automatización para el proyecto Veloren

---

## Contexto

Veloren es un juego RPG voxel multijugador en Rust (~28 crates, ~nightly-2025-09-08). El desarrollador trabaja en todos los aspectos del proyecto: mecánicas de gameplay, generación procedural de mundo, debugging de bugs y revisión de código. Las compilaciones incrementales toman ~10 segundos.

Se elige **Opción C — Skills por flujo + hooks mínimos** porque el costo de las iteraciones es bajo pero el costo cognitivo de recordar qué comandos correr en cada contexto es alto.

---

## 1. Project Skills (5 skills)

Ubicación: `.claude/skills/veloren-<nombre>/SKILL.md`

### 1.1 `veloren-run`

**Propósito:** Lanzar el cliente o servidor con el entorno correcto.

**Responsabilidades:**
- Setear `VELOREN_ASSETS="$(pwd)/assets"` siempre
- Conocer los binarios disponibles: `veloren-voxygen` (cliente GUI), `veloren-server-cli` (servidor headless)
- Conocer los aliases de `.cargo/config.toml`: `cargo server`, `cargo test-server`, `cargo tracy-server`, `cargo tracy-voxygen`, `cargo dbg-voxygen`
- Guiar entre modo dev (hot-reloading activado por defecto) y modo release (`--no-default-features --features default-publish`)
- Para single-player: `cargo run --bin veloren-voxygen` (embeds server via `singleplayer` feature)
- Para servidor dedicado con profiling: `cargo tracy-server`

**No hace:** compilar desde cero, gestionar configuración del servidor.

### 1.2 `veloren-dev`

**Propósito:** Guía estructurada para implementar nuevas mecánicas o modificar las existentes.

**Flujo:**
1. Identificar el tipo de cambio:
   - **Nuevo componente ECS** → `common/src/comp/` → registrar en `common-state/src/state.rs`
   - **Nuevo sistema compartido** → `common/systems/src/` → registrar en `common/systems/src/lib.rs`
   - **Nuevo sistema servidor** → `server/src/sys/` → registrar en `server/src/sys/mod.rs`
   - **Nueva habilidad de combate** → `common/src/comp/ability.rs` + estado en `character_state.rs`
   - **Comportamiento NPC** → `server/agent/src/action_nodes.rs` o `attack.rs`
   - **Nuevo comando admin** → enum en `common/src/cmd.rs` + handler en `server/src/cmd.rs`
2. Revisar patrones existentes similares antes de escribir código nuevo
3. Invocar `superpowers:test-driven-development` para escribir tests primero
4. Correr tests del crate afectado: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-<crate>`
5. Verificar con `cargo check -p veloren-<crate>` antes de compilar todo

**Archivos de referencia por área:**
- Combate: `common/src/comp/ability.rs` (142KB), `character_state.rs` (65KB)
- NPC AI: `server/agent/src/attack.rs` (361KB), `action_nodes.rs` (103KB)
- Inventario: `common/src/comp/inventory/` (loadout_builder.rs 61KB)
- Física: `common/systems/src/phys/mod.rs`

### 1.3 `veloren-debug`

**Propósito:** Debugging sistemático de bugs en el juego, con contexto de ECS.

**Flujo (basado en `superpowers:systematic-debugging`):**
1. **Reproducir** — identificar el comando admin o secuencia para reproducir (`/debug`, `/give`, `/tp`, etc.)
2. **Localizar en ECS** — ¿es un componente? ¿un sistema? ¿un evento? ¿un mensaje de red?
   - Componente incorrecto → buscar en `common/src/comp/`
   - Sistema que no corre → revisar dependencias en dispatcher
   - Evento no emitido → buscar en `common/src/event.rs` y handlers en `server/src/events/`
3. **Instrumentar** — agregar `tracing::debug!()` o `tracing::warn!()` spans temporales
4. **Profiling** — si es performance: `cargo tracy-server` o `cargo tracy-voxygen` con Tracy profiler
5. **Gizmos visuales** — para bugs de física/posición: `common/src/comp/gizmos.rs`
6. **Logs** — servidor en `userdata/server/logs/`, cliente en `userdata/client/logs/`

**Comandos de debugging en-game:**
- `/debug` — toggle de debug rendering
- `/give <item>` — spawnear items para testear
- `/tp <x> <y> <z>` — teletransporte para reproducir bugs geográficos
- `/entity` — inspeccionar estado de entidades

**Invoca:** `superpowers:systematic-debugging` antes de proponer cualquier fix.

### 1.4 `veloren-review`

**Propósito:** Revisión de código completa antes de mergear.

**Flujo:**
1. **Formato:** `cargo fmt --all -- --check` → si falla, `cargo fmt --all` y revisar diff
2. **Lint CI completo:** `cargo ci-clippy` (todos los targets)
3. **Lint voxygen publish:** `cargo ci-clippy2` (sin hot-reloading)
4. **Verificar patrones ECS:**
   - Componentes nuevos: ¿están registrados en `common-state/src/state.rs`?
   - Sistemas nuevos: ¿están en el dispatcher con dependencias correctas?
   - Recursos nuevos: ¿están en `common/src/resources.rs`?
5. **Invocar** `superpowers:requesting-code-review` para análisis profundo del diff

**No hace:** merge, push, ni modificar código — solo verifica y reporta.

### 1.5 `veloren-worldgen`

**Propósito:** Iterar sobre generación procedural de mundo, rtsim y sitios.

**Etapas de generación (en orden):**
1. `world/src/sim/` — WorldSim: erosión, ríos, biomas, cuevas (profile: `no_overflow`)
2. `world/src/civ/` — WorldCiv: asentamientos, rutas comerciales, facciones
3. `world/src/site/` — Sitios individuales: ciudades, mazmorras, castillos
4. `world/src/layer/` — Capas: árboles, pasto, sprites
5. `rtsim/` — Simulación de largo plazo: NPCs, economía, conflictos

**Flujo de iteración:**
```
cambio en world/src/ o rtsim/
→ cargo check -p veloren-world (o veloren-rtsim)
→ cargo server (arrancar servidor con worldgen)
→ volar al área afectada (/tp o airship)
→ observar resultado
→ ajustar parámetros (RON en assets/world/ o consts en código)
```

**Herramientas específicas:**
- `cargo dot-recipes` — grafo de recetas (graphviz)
- `cargo dot-skills` — grafo de habilidades
- Profile `no_overflow` para world-gen math (evita overflow checks que ralentizan)
- Feature `airship_maps` para visualizar rutas de airship
- Feature `bin_compression` para benchmark de compresión de chunks

**Archivos clave:**
- `world/src/lib.rs` (31KB) — struct World, etapas
- `world/src/column.rs` (56KB) — generación de columnas por chunk
- `rtsim/src/lib.rs` — Event/Rule system, diseño del motor de simulación
- `assets/world/` — configuración RON de parámetros de generación

---

## 2. Hooks (4 hooks)

Configurados en `.claude/settings.local.json` del proyecto Veloren.

### 2.1 Stop — Recordatorio de lint

**Trigger:** Cuando Claude termina de responder
**Acción:** Verifica si hay archivos `.rs` modificados no commiteados. Si los hay, imprime recordatorio.
**Tokens consumidos:** Ninguno (corre después de que Claude para)

```bash
cd /Users/mgrinberg/Workspace/RustroverProjects/veloren && \
git diff --name-only HEAD 2>/dev/null | grep -q '\.rs$' && \
echo "⚠  Hay archivos .rs modificados — recordá correr: cargo ci-clippy" || true
```

### 2.2 PreToolUse — Gate pre-commit

**Trigger:** Antes de ejecutar `git commit`
**Acción:** Corre `cargo fmt --all -- --check`. Si falla, bloquea el commit y muestra qué archivos necesitan formato.
**Tokens consumidos:** Solo cuando se intenta commitear (infrecuente)

```bash
source "$HOME/.cargo/env" && \
cd /Users/mgrinberg/Workspace/RustroverProjects/veloren && \
cargo fmt --all -- --check 2>&1 || \
(echo "BLOQUEADO: Corregí el formato antes de commitear. Corré: cargo fmt --all" && exit 1)
```

### 2.3 PreToolUse — Guard rm crítico

**Trigger:** Bash commands que contienen `rm` sobre rutas críticas
**Protege:** `assets/`, archivos `.sqlite`, `server/src/persistence/`, `userdata/`
**Acción:** Si el comando rm toca estas rutas, bloquea y requiere confirmación explícita del usuario

Matcher: `Bash(rm * assets*)`, `Bash(rm *.sqlite*)`, `Bash(rm * persistence*)`, `Bash(rm * userdata*)`

### 2.4 PreToolUse — Guard SQLite

**Trigger:** Comandos Bash con operaciones de escritura en SQLite
**Protege:** Cualquier modificación directa a la base de datos de persistencia del juego
**Detecta:** `sqlite3 *.sqlite`, comandos con `DROP TABLE`, `DELETE FROM`, `UPDATE` directos
**Acción:** Muestra el comando completo al usuario y requiere aprobación antes de ejecutar

Matcher: `Bash(sqlite3 *)`, `Bash(* DROP TABLE *)`, `Bash(* DELETE FROM *)`

---

## 3. Flujo de Trabajo Integrado

### Feature nueva (mecánica de gameplay)
```
veloren-dev → TDD → implementar → cargo check -p <crate> → veloren-run (probar) → veloren-review → commit (gate fmt) → PR
```

### Bug encontrado
```
veloren-debug (systematic-debugging) → reproducir → instrumentar → fix → verificar → veloren-review → commit
```

### Iteración de world-gen
```
veloren-worldgen → modificar sim/civ/site → cargo check -p veloren-world → veloren-run (servidor) → observar → ajustar
```

### Merge a main
```
veloren-review → cargo ci-clippy + ci-clippy2 → superpowers:requesting-code-review → fix feedback → PR
```

---

## 4. Skills de Superpowers a Usar Proactivamente

| Skill | Cuándo usarlo |
|-------|--------------|
| `systematic-debugging` | Ante cualquier bug antes de proponer fixes |
| `test-driven-development` | Para nuevos componentes ECS o sistemas |
| `dispatching-parallel-agents` | Cuando el trabajo abarca múltiples crates independientes |
| `using-git-worktrees` | Para features que necesitan aislamiento del workspace actual |
| `verification-before-completion` | Antes de declarar que algo "está listo" o "los tests pasan" |
| `requesting-code-review` | Siempre antes de mergear a main |
| `finishing-a-development-branch` | Al terminar una feature completa |

---

## 5. Lo que NO se automatiza

- Compilación completa en cada edición (10s es manejable, tokens innecesarios)
- Push automático a ningún remote
- Merge automático a ninguna branch
- Modificaciones a la base de datos sin aprobación del usuario

---

## Resumen de Archivos a Crear/Modificar

| Archivo | Acción |
|---------|--------|
| `.claude/skills/veloren-run/SKILL.md` | Crear |
| `.claude/skills/veloren-dev/SKILL.md` | Crear |
| `.claude/skills/veloren-debug/SKILL.md` | Crear |
| `.claude/skills/veloren-review/SKILL.md` | Crear |
| `.claude/skills/veloren-worldgen/SKILL.md` | Crear |
| `.claude/settings.local.json` | Modificar (agregar 4 hooks) |
