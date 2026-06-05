# Fase 2 Task 7 — Block-unit Constants Audit

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Scale all remaining block-unit constants in `base_accel()`, NPC `dimensions()`, server `max_view_distance`, and world-gen files by `HIRES_SCALE` so that the `terrain-hires` feature flag produces physically correct behavior for all entities (not just the humanoid player).

**Architecture:** Same pattern used in Tasks 1-5: multiply each numeric constant that is expressed in "blocks" by `common::consts::HIRES_SCALE` (= 2.0 with `terrain-hires`, = 1.0 otherwise). No new types or abstractions needed. Save migration is deferred to a separate task.

**Tech Stack:** Rust nightly, `common` crate, `server` crate. Feature flag `terrain-hires` already wired.

**Context:** Tasks 1-5 of the Fase 2 plan are complete (HEAD `18fb5c97b2`). The full Fase 2 plan lives at `docs/superpowers/plans/2026-06-05-fase2-block-scale.md`.  
To test with the flag active: `cargo run --bin veloren-voxygen --features veloren-voxygen/terrain-hires`

---

## File Map

| File | Change |
|------|--------|
| `common/src/states/utils.rs` | Multiply every numeric literal in `base_accel()` by `HIRES_SCALE` |
| `common/src/comp/body/mod.rs` | Multiply every `Vec3::new(x, y, z)` literal in `dimensions()` (except Humanoid — already done) by `HIRES_SCALE` |
| `server/src/settings/mod.rs` | Scale `max_view_distance: Some(65)` default |

---

## Task 1: Scale `base_accel()` for all body types

`base_accel()` is in `common/src/states/utils.rs` line 53. Every numeric literal is in blocks/s². They must double so real-world acceleration stays the same when blocks are half-size. `HIRES_SCALE` is already imported via line 25: `use common::consts::{..., HIRES_SCALE, ...}`.

**Files:**
- Modify: `common/src/states/utils.rs:53-205`

- [ ] **Step 1: Read the current function**

```bash
sed -n '53,206p' common/src/states/utils.rs
```

Expected: Match arms with bare float literals like `=> 100.0,`, `=> 30.0,`, etc. down to `Body::Plugin(body) => body.base_accel()`.

- [ ] **Step 2: Apply the transformation**

The pattern is: every `=> <number>.0,` and `=> <number>.0` that appears as the return of a `base_accel()` match arm needs `* HIRES_SCALE` appended.

Use a targeted sed replacement (this does NOT touch `body.base_accel()` because that has no literal):

```bash
# Preview first — should show all the float literals in that function
sed -n '53,205p' common/src/states/utils.rs | grep -n "=> [0-9]*\.[0-9]*,"
```

Expected output: lines like `5:            Body::Humanoid(_) => 100.0,`

Now apply. Because the match spans lines 53-205, and all `=> X.0,` patterns in that range are base_accel arms, we can safely do a scoped replacement. Open the file in your editor or use the Edit tool to:

Replace every line of the form `=> N.0,` (where N is a number) in lines 53-205 with `=> N.0 * HIRES_SCALE,`.

The exact substitution for each line:
```
Body::Humanoid(_) => 100.0,
```
→
```
Body::Humanoid(_) => 100.0 * HIRES_SCALE,
```

Repeat for every species entry. The entries that need changing (values from reading the file):

**QuadrupedSmall** (line ~58-75):
- Turtle: `30.0` → `30.0 * HIRES_SCALE`
- Axolotl/Pig/Sheep/Truffler/Fungome: `70.0` → `70.0 * HIRES_SCALE`
- Goat: `80.0` → `80.0 * HIRES_SCALE`
- Raccoon/Porcupine/Beaver/Quokka: `100.0` → `100.0 * HIRES_SCALE`
- Frog/Cat: `150.0` → `150.0 * HIRES_SCALE`
- Rabbit: `110.0` → `110.0 * HIRES_SCALE`
- MossySnail: `20.0` → `20.0 * HIRES_SCALE`
- `_ => 125.0` → `_ => 125.0 * HIRES_SCALE`

**QuadrupedMedium** (line ~76-115):
All values in that arm: `100.0`, `110.0`, `85.0`, `105.0`, `130.0`, `115.0`, `75.0`, `60.0`, `120.0`, `140.0`, `80.0`, `90.0`, `95.0`, `155.0`, `150.0`, `70.0` — all → append `* HIRES_SCALE`

**BipedLarge** (line ~116-129):
`100.0`, `90.0`, `60.0`, `130.0`, `110.0`, `45.0`, `50.0` → all `* HIRES_SCALE`; `_ => 80.0` → `_ => 80.0 * HIRES_SCALE`

**Flat values** (line ~130-135):
- `BirdMedium: 80.0` → `80.0 * HIRES_SCALE`
- `FishMedium: 80.0` → `80.0 * HIRES_SCALE`
- `Dragon: 250.0` → `250.0 * HIRES_SCALE`
- `BirdLarge: 110.0` → `110.0 * HIRES_SCALE`
- `FishSmall: 60.0` → `60.0 * HIRES_SCALE`

**BipedSmall** (line ~135-140):
`65.0`, `100.0`, `70.0` → `* HIRES_SCALE`; `_ => 80.0` → `_ => 80.0 * HIRES_SCALE`

**Object/Item** (lines ~141-142): `0.0` — skip (zero times anything is zero, no change needed)

**Golem** (line ~143-147):
`120.0`, `100.0` → `* HIRES_SCALE`; `_ => 60.0` → `* HIRES_SCALE`

**Theropod** (line ~148-155):
`110.0`, `75.0`, `115.0` → `* HIRES_SCALE`; `_ => 125.0` → `* HIRES_SCALE`

**QuadrupedLow** (line ~156-181):
`60.0` (×3), `65.0`, `85.0` (×2), `130.0`, `100.0` (×6), `70.0` (×3), `80.0`, `125.0` (×2), `140.0`, `110.0`, `120.0` (×2) → all `* HIRES_SCALE`

**Ship** (line ~182-184):
- `Carriage: 40.0` → `40.0 * HIRES_SCALE`
- `Train: 9.0` → `9.0 * HIRES_SCALE`
- `_ => 0.0` — skip

**Arthropod** (line ~185-199):
`85.0`, `95.0`, `115.0`, `80.0`, `65.0`, `80.0`, `70.0`, `90.0`, `70.0` (×3), `75.0` → all `* HIRES_SCALE`

**Crustacean** (line ~200-203):
`80.0` (×2), `120.0` → all `* HIRES_SCALE`

**Plugin** (line ~204): `body.base_accel()` — skip (no literal)

**Humanoid** (line ~57): `100.0` → `100.0 * HIRES_SCALE`

- [ ] **Step 3: Compile check both modes**

```bash
source "$HOME/.cargo/env" && cargo build -p veloren-common 2>&1 | tail -5
```
Expected: `Finished dev profile` (no errors)

```bash
cargo build -p veloren-common --features veloren-common/terrain-hires 2>&1 | tail -5
```
Expected: `Finished dev profile`

- [ ] **Step 4: Commit**

```bash
git add common/src/states/utils.rs
git commit -m "feat(terrain-hires): scale base_accel() for all body types by HIRES_SCALE"
```

---

## Task 2: Scale `dimensions()` for all NPC body types

`dimensions()` is in `common/src/comp/body/mod.rs` starting at line 572. The `Humanoid` arm (line 681-684) is already correct. Every other arm has hardcoded `Vec3::new(x, y, z)` in block units that must scale.

**Files:**
- Modify: `common/src/comp/body/mod.rs:572-840`

The `HIRES_SCALE` import is not yet present in this file — check with:

```bash
grep -n "HIRES_SCALE\|use crate::consts\|use common::consts" common/src/comp/body/mod.rs | head -5
```

If no match, the existing Humanoid arm uses `crate::consts::HIRES_SCALE` inline (confirmed at line 683: `0.8 * crate::consts::HIRES_SCALE`). We'll use the same `crate::consts::HIRES_SCALE` inline form for all new changes to match the existing pattern.

- [ ] **Step 5: Read the full dimensions() function**

```bash
sed -n '572,845p' common/src/comp/body/mod.rs
```

Verify the structure: one `Vec3::new(...)` per species per body type variant.

- [ ] **Step 6: Scale BipedLarge dimensions**

All `Vec3::new(a, b, c)` in the `Body::BipedLarge` arm (lines ~574-613) need to become `Vec3::new(a, b, c) * crate::consts::HIRES_SCALE`. Since `Vec3<f32>` implements `Mul<f32>`, the scalar multiplication applies element-wise.

Example — change:
```rust
biped_large::Species::Cyclops => Vec3::new(5.6, 3.0, 8.0),
```
to:
```rust
biped_large::Species::Cyclops => Vec3::new(5.6, 3.0, 8.0) * crate::consts::HIRES_SCALE,
```

Apply to all ~31 species entries in BipedLarge.

- [ ] **Step 7: Scale BipedSmall dimensions**

Same pattern for `Body::BipedSmall` arm (lines ~614-648), ~28 species entries.

Example:
```rust
biped_small::Species::Gnarling => Vec3::new(1.0, 0.75, 1.4),
```
→
```rust
biped_small::Species::Gnarling => Vec3::new(1.0, 0.75, 1.4) * crate::consts::HIRES_SCALE,
```

- [ ] **Step 8: Scale BirdLarge dimensions**

`Body::BirdLarge` arm (lines ~649-658), 6 species entries.

- [ ] **Step 9: Scale Dragon dimensions**

`Body::Dragon` arm (lines ~659-661), 1 entry:
```rust
dragon::Species::Reddragon => Vec3::new(16.0, 10.0, 16.0),
```
→
```rust
dragon::Species::Reddragon => Vec3::new(16.0, 10.0, 16.0) * crate::consts::HIRES_SCALE,
```

- [ ] **Step 10: Scale FishMedium + FishSmall dimensions**

`Body::FishMedium` (2 entries) and `Body::FishSmall` (2 entries).

- [ ] **Step 11: Scale Golem dimensions**

`Body::Golem` arm (lines ~670-679), 9 species entries.

- [ ] **Step 12: Skip Humanoid — already done**

Lines 681-684 already have `0.8 * crate::consts::HIRES_SCALE` and humanoid `height()` already scales. Skip.

- [ ] **Step 13: Scale QuadrupedMedium dimensions**

`Body::QuadrupedMedium` arm (lines ~687-727), ~32 entries.

- [ ] **Step 14: Scale QuadrupedSmall dimensions**

`Body::QuadrupedSmall` arm (lines ~728-759), ~28 entries.

- [ ] **Step 15: Scale QuadrupedLow dimensions**

`Body::QuadrupedLow` arm (lines ~760-786), ~23 entries.

- [ ] **Step 16: Skip Ship — delegates to `ship.dimensions()`**

Line 787: `Body::Ship(ship) => ship.dimensions()`. That function is in a separate file. Leave it for a follow-up unless it also contains hardcoded block units — check:

```bash
grep -n "Vec3::new\|HIRES_SCALE" common/src/comp/body/ship.rs | head -10
```

If it has bare `Vec3::new(...)` literals, add scaling there too. If it delegates or uses relative sizes, skip.

- [ ] **Step 17: Scale Theropod dimensions**

`Body::Theropod` arm (lines ~788-799), 9 entries.

- [ ] **Step 18: Scale Arthropod dimensions**

`Body::Arthropod` arm (lines ~800-813), 13 entries.

- [ ] **Step 19: Scale BirdMedium dimensions**

`Body::BirdMedium` arm (lines ~815-834), 18 entries.

- [ ] **Step 20: Scale Crustacean dimensions**

`Body::Crustacean` arm (lines ~835-838), 3 entries.

- [ ] **Step 21: Compile check both modes**

```bash
source "$HOME/.cargo/env" && cargo build -p veloren-common 2>&1 | tail -5
```
Expected: `Finished dev profile`

```bash
cargo build -p veloren-common --features veloren-common/terrain-hires 2>&1 | tail -5
```
Expected: `Finished dev profile`

- [ ] **Step 22: Commit**

```bash
git add common/src/comp/body/mod.rs
git commit -m "feat(terrain-hires): scale NPC dimensions() by HIRES_SCALE for all body types"
```

---

## Task 3: Scale server `max_view_distance` default

`server/src/settings/mod.rs` line 229: `max_view_distance: Some(65)`. This is the server-side cap in chunk units. With half-size blocks, the same real-world view distance requires twice as many chunks.

`server` depends on `veloren-common` which has the `terrain-hires` feature, so we can use `veloren_common::consts::HIRES_SCALE`.

**Files:**
- Modify: `server/src/settings/mod.rs:229`

- [ ] **Step 23: Check existing imports in settings/mod.rs**

```bash
grep -n "^use\|veloren_common" server/src/settings/mod.rs | head -15
```

Expected: a `use veloren_common::...` or `use common::...` somewhere near the top.

- [ ] **Step 24: Update max_view_distance default**

Find in `server/src/settings/mod.rs`:
```rust
max_view_distance: Some(65),
```

Change to:
```rust
max_view_distance: Some((65.0 * veloren_common::consts::HIRES_SCALE) as u32),
```

If `veloren_common` is not the crate name used in server, check `server/Cargo.toml`:
```bash
grep "veloren-common\|common" server/Cargo.toml | head -5
```

Adjust the path accordingly (might be `common::consts::HIRES_SCALE` if re-exported).

- [ ] **Step 25: Also check the second occurrence at line 311**

Line 311 has `max_view_distance: None` (the `load` fallback). `None` means "no cap" — no scaling needed.

- [ ] **Step 26: Compile check server crate**

```bash
source "$HOME/.cargo/env" && cargo build -p veloren-server 2>&1 | tail -5
```
Expected: `Finished dev profile`

```bash
cargo build -p veloren-server --features veloren-server/terrain-hires 2>&1 | tail -5
```

If `terrain-hires` is not yet propagated to `server/Cargo.toml`, add it now:
```bash
grep -n "terrain-hires\|\[features\]" server/Cargo.toml | head -5
```

If missing:
```toml
# In server/Cargo.toml [features]:
terrain-hires = ["veloren-common/terrain-hires"]
```

Then re-run the compile check.

- [ ] **Step 27: Commit**

```bash
git add server/src/settings/mod.rs server/Cargo.toml
git commit -m "feat(terrain-hires): scale server max_view_distance default by HIRES_SCALE"
```

---

## Task 4: World gen constants audit

This is a broad audit. The goal is to find numeric literals in `world/src/` that represent heights or distances in block units and multiply them by `HIRES_SCALE`.

**Files:**
- Modify: possibly `world/src/column.rs`, `world/src/sim/mod.rs`, `world/src/layer/cave.rs`, `world/src/layer/scatter.rs`, `world/src/site/` files

**Important:** `sea_level` and `mountain_scale` in `world/src/config.rs` were already scaled in Task 4 of the original plan (commit `994d2b8cb0`). Do NOT re-scale those.

- [ ] **Step 28: Find candidate numeric block-unit literals in world gen**

```bash
grep -rn "HIRES_SCALE\|use common::consts\|use veloren_common::consts" world/src/ | grep -v ".rs:.*//\|test" | head -20
```

This shows where HIRES_SCALE is already applied. Next, find constants that look like heights (order of magnitude 10–10000) and are NOT already scaled:

```bash
grep -rn "const.*: f32\|const.*: i32\|const.*: f64" world/src/ | grep -v "//\|HIRES\|probability\|temperature\|humidity\|color\|noise\|freq\|scale.*=.*1\." | head -40
```

- [ ] **Step 29: Check world/src/sim/mod.rs for unscaled height constants**

```bash
grep -n "const\|sea_level\|mountain\|height\|altitude\|HIRES" world/src/sim/mod.rs | head -30
```

For each constant found: assess whether it represents a block height/distance (needs scaling) or a dimensionless parameter (probabilities, noise weights — no scaling). Add `* HIRES_SCALE` to those that do.

- [ ] **Step 30: Check world/src/layer/cave.rs**

```bash
grep -n "const\|height\|depth\|radius\|HIRES" world/src/layer/cave.rs | head -20
```

Cave sizes and depths are in blocks → scale. Cave probabilities and noise parameters → don't scale.

- [ ] **Step 31: Check world/src/layer/scatter.rs**

```bash
grep -n "const\|height\|offset\|HIRES" world/src/layer/scatter.rs | head -20
```

Tree placement heights and sprite offsets are in blocks → scale.

- [ ] **Step 32: Compile check world crate with flag**

```bash
source "$HOME/.cargo/env" && cargo build -p veloren-world --features veloren-world/terrain-hires 2>&1 | tail -10
```
Expected: `Finished dev profile`

- [ ] **Step 33: Commit world gen changes**

```bash
git add world/src/
git commit -m "feat(terrain-hires): scale world gen height/distance constants by HIRES_SCALE"
```

---

## Task 5: Final integration clippy check

- [ ] **Step 34: Run clippy for all targets (normal mode)**

```bash
source "$HOME/.cargo/env" && cargo ci-clippy -- -D warnings 2>&1 | tail -20
```
Expected: `Finished` with no warnings.

- [ ] **Step 35: Run clippy for voxygen publish profile**

```bash
cargo ci-clippy2 -- -D warnings 2>&1 | tail -20
```
Expected: `Finished` with no warnings.

Fix any lint warnings before proceeding.

- [ ] **Step 36: Verify terrain-hires compiles cleanly**

```bash
cargo build -p veloren-voxygen --features veloren-voxygen/terrain-hires 2>&1 | tail -5
```
Expected: `Finished dev profile`

---

## Known gaps (out of scope for this plan)

- **Save migration**: existing `userdata/` saves have block coordinates in old units. Loading with `terrain-hires` places the character at half the correct height. Deferred — needs a versioned save format (separate plan).
- **Ship `dimensions()`**: if `common/src/comp/body/ship.rs` has bare block-unit Vec3 literals, they need the same treatment. Deferred until confirmed.
- **World gen site floors**: `world/src/site/` building floor heights may need scaling. Covered by Task 4 audit but depth depends on what's found.
- **Asset `.ron` scale fields**: `.vox` model manifests have `scale` fields. These are *visual* scales, not collider sizes — they may or may not need adjustment. Visual-only change, deferred to post-Fase-3 polish.
