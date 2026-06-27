# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Interaction Convention — Fill-in Worksheets (Matias ⇄ Claude)

Whenever you need Matias to **make decisions, choose between options, confirm renames/changes, or supply information**, do **not** scatter the questions through prose or rely only on `AskUserQuestion`. Instead present a **plain-text fill-in worksheet** Matias can copy into Sublime Text, complete offline, and paste back whole — easy for him to fill, unambiguous for you to parse, with tables that never break alignment.

Rules (full spec + canonical example: `docs/design/conventions/fill-in-worksheets.md`):
- Wrap the entire worksheet in a fenced code block so it renders monospace; align all columns and `->` arrows.
- Header box with `=====` borders stating what it is and what happens on confirm; sections numbered and split by `------` rules.
- **Bulk confirmations in a BLOCK** with one global `[DG] decisión global:` + `excepciones:` field ("OK a todos" once), and "(se mantienen / ya confirmados …)" notes so he sees what is NOT changing.
- **Real decisions as `[Q1]`, `[Q2]`, …**, each with a `decisión:` blank line; coinages get **OPCIÓN A / OPCIÓN B**, a `[pick]`, and a free `propio` column.
- Final action section `[P1] … (SI / NO)`; close with `FIN. Devolveme el bloque completado.`

This is the default for any multi-decision / bulk request (`AskUserQuestion` only for 1–4 quick structural forks).

## Toolchain

Nightly Rust is required (pinned in `rust-toolchain`). The project uses the 2024 edition. The `specs` ECS crate requires nightly.

## Commands

```bash
# Run the game client (hot-reloading enabled by default in dev builds)
cargo run --bin veloren-voxygen

# Run the server
cargo run --bin veloren-server-cli

# Tests require the assets path
VELOREN_ASSETS="$(pwd)/assets" cargo test

# Single crate test
VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common

# Lint (matches CI exactly)
cargo clippy --all-targets --locked \
  --features="bin_cmd_doc_gen,bin_compression,bin_csv,bin_graphviz,bin_bot,bin_asset_migrate,asset_tweak,bin,stat,cli" \
  -- -D warnings

# Clippy for voxygen publish profile (no hot-reloading)
cargo clippy -p veloren-voxygen --locked --no-default-features --features="default-publish" -- -D warnings

# Format check
cargo fmt --all -- --check

# Release build (no hot-reloading, with LTO)
cargo build --release --no-default-features --features default-publish
```

## Workspace Architecture

The project separates into four layers:

**Executables**
- `voxygen/` — GUI client. Owns rendering (wgpu), windowing (winit), UI (egui + conrod), audio, and asset hot-reloading. The `hot-reloading` feature (on by default in dev) loads animation and agent code as dynamic libraries via `common-dynlib`.
- `server-cli/` — Headless server binary wrapping the `server` crate.

**Game logic**
- `server/` — Authoritative game state: ECS tick, player connections, persistence, economy.
- `client/` — Client-side game logic and networking (no graphics).
- `server-agent/` — NPC AI behavior, compiled as a hot-reloadable dylib in dev.
- `rtsim/` — Long-running world simulation (NPC migrations, factions, civilization events).
- `world/` — Procedural world generation: terrain, sites (towns/dungeons), caves, trees.

**Common layer** (`common/` + sub-crates)
- `common/` — Core game types: components, items, recipes, combat formulas, terrain chunks.
- `common-state/` — ECS world setup; integrates plugins; shared between client and server.
- `common-systems/` — ECS systems (physics, buffs, projectiles, etc.) run on both sides.
- `common-net/` — Network message types and compression.
- `common-assets/` — Asset loading abstraction over the `assets_manager` crate.
- `common-ecs/` — ECS utility traits on top of `specs`.

**Network**
- `network/` — Low-level multiplayer transport (TCP, QUIC via Quinn, optional metrics).
- `network-protocol/` — Wire format and message serialization.

## ECS Pattern

The codebase uses `specs`. Components live in `common/src/comp/`, resources in `common/src/resources.rs`. Systems in `common-systems/` are registered in `common-state/`. Server-only systems are in `server/src/sys/`. Always check existing comp/system patterns before adding new ones.

## Assets

All game data (voxel models, audio, i18n strings, configs) lives in `assets/`. The build reads `VELOREN_ASSETS` at runtime; in dev it defaults to `$(pwd)/assets`. Asset configs use RON format. Items, recipes, and entity configs are data-driven and live under `assets/common/`.

The large **binary** assets are stored via Git LFS on a self-hosted VPS store, **not** on GitHub — see **Git LFS & Binary Assets (the VPS)** below.

## Hot-reloading

In dev builds, `voxygen-anim` and `server-agent` are compiled as `cdylib` crates and loaded at runtime. Changes to animation or AI code reload without restarting. This is gated by the `hot-reloading` feature; the `default-publish` feature set disables it for release builds.

## Features of Note

- `tracy` — Enables Tracy profiler integration across crates.
- `asset_tweak` — Allows runtime asset value tweaking for balancing.
- `simd` — Enables SIMD optimizations in server-cli.
- `bin_*` — Various utility binaries (CSV export, graph generation, bot, asset migration).

## Documentation & Git Policy

**Where docs live — two repos, one working tree:**
- Design docs (specs, plans, task boards) live in `docs/design/`, which is a **separate, private git repo** (`Matute289/xindeler-design`) nested inside this one and gitignored here. Commit and push design docs from inside `docs/design/` — never into this (public) repo.
  - Specs → `docs/design/specs/`, implementation plans → `docs/design/plans/`, task boards → `docs/design/tasks/` (index: `00-task-board.md`).
- Lore canon (markdown) lives at `docs/design/lore/` in the private design repo. `docs/lore/` is a legacy path kept gitignored as a guard — never create files there.
- `.superpowers/` (brainstorm scratch) and `graphify-out/` are local-only and gitignored; never commit them anywhere. Brainstorm conclusions belong as a spec/plan in `docs/design/`.
- The `gitlab` remote is the fetch-only upstream (push disabled); never push to it.

**Branch protection (public repo `Matute289/xindeler`):**
- `main` and `development` are protected: no direct pushes (admins included), no force-pushes, no deletion. All changes land via PR with 1 approval.
- AI agents must NEVER merge or approve PRs, push to `main`/`development`, or touch branch-protection settings. Workflow: branch off `development` → commit → push branch → open PR with base `development` → stop and report. Only Matias reviews and merges.

## Git LFS & Binary Assets (the VPS) — IMPORTANT

Large binary assets (`.vox`, `.png`/`.jpg`/`.jpeg`, `.ogg`/`.wav`, `.ttf`, `.ico`, `.obj`/`.blend`, `assets/world/map/*.bin`, etc. — the full list is `.gitattributes`) are **NOT stored on GitHub**. They live on a self-hosted Git LFS store on the VPS. GitHub holds only code, RON/i18n text, and tiny **LFS pointer files**.

**Topology — three sources, one working tree:**
- **GitHub public** (`Matute289/xindeler`, `origin`) — code + RON/i18n + LFS pointers. No blobs.
- **VPS** (`greenmountain.dev:/srv/git-lfs/repos/xindeler.git`) — the actual binary blobs, served by `git-lfs-transfer` over **pure SSH** (no HTTP server, no Caddy). Private (SSH-key auth). It is the **single copy** of the binaries, so it must be backed up server-side. Server-side setup notes live in the private `MyServerVPS` repo (`git-lfs/`).
- **GitHub private** (`Matute289/xindeler-design`, nested at `docs/design/`) — design/lore.

**How it's wired:**
- `.lfsconfig` (committed) sets `lfs.url = ssh://mgrinberg@greenmountain.dev/srv/git-lfs/repos/xindeler.git`. Every clone reads it, so all LFS push/fetch goes to the VPS — never GitHub.
- `.gitattributes` tracks **only binaries**. RON/i18n and all text stay as normal git files — data-driven content travels with the code; never LFS-track it.
- Requires **git-lfs ≥ 3.0** on every client (there is no HTTP fallback) plus SSH access to the VPS to fetch/push blobs.

**Rules going forward:**
- **Never re-introduce GitHub LFS.** No workflow may `actions/checkout` with `lfs: true` against GitHub, nor `git lfs push … github`. Route LFS to the VPS: local work uses the committed `.lfsconfig`; CI must add a `Setup SSH` step with `secrets.VPS_SSH_KEY` and pull from the VPS (see `publish-docker.yml` for the pattern).
- To add new binary assets, just commit them normally — the pre-push hook sends blobs to the VPS automatically; GitHub gets only the pointer.
- Without VPS SSH access, a clone gets code + pointers but **not** the real binaries — this is the intended privacy boundary (assets stay private).

## Releases & CI

**Where each build runs:**
- **Code CI** (build / check / test / lint on PRs) → **GitHub Actions** (public repo = free, unlimited minutes). It must **not** pull LFS — compilation and tests don't need the binary assets.
- **Server release** → built **on the VPS** (where the assets are local), not on GitHub Actions. `release.yml` triggers on a `v*` tag push, SSHes to the VPS with `secrets.VPS_SSH_KEY`, and runs `/srv/git-lfs/scripts/build-release.sh <tag>` → produces `/srv/git-lfs/releases/xindeler-server-<tag>.tar.gz`.
- **Docker image** (`publish-docker.yml`, manual) → pulls only the asset dirs the image bundles (`assets/common,server,world`) from the VPS, builds `veloren-server-cli`, pushes to GHCR.
- **Client release** (voxygen desktop installer + Airshipper) → **deferred** to the first client release; study Veloren's packaging then. The shipped client necessarily bundles its assets (players have them locally) — "private" means private in source control, not in the shipped binary.

**GitHub Actions minutes:** the 2,000-minute quota is for **private** repos only; the public `xindeler` repo runs Actions for free. Heavy Rust builds run on the VPS anyway, so they don't consume GitHub minutes.

## Upstream Sync (GitLab Veloren)

Xindeler is a fork of `gitlab veloren/veloren` (the `gitlab` remote — fetch-only, never push). To pull upstream `master` and update without breaking or overwriting Xindeler's work:

- **Use the `GitlabMasterMerger` skill** together with the `upstream-sync.yml` workflow. They bring upstream changes into a **review branch** (`upstream/review-…`) and integrate via **PR** — they do **not** force-push `main`/`development`.
- ⚠️ **Never hard-mirror** upstream over our branches. (The old `mirror.yml` did `git push --force master→main` and was removed for exactly this reason; branch protection blocks it anyway.)
- Upstream brings its own LFS binaries — these route to the **VPS** via `.lfsconfig`, never to GitHub.
- After a sync, run the lint/test commands above and resolve conflicts so Xindeler customizations (classes, races, magic, lore-driven assets, CI/LFS config, etc.) are preserved — upstream must never clobber them.

## Build Profiles

Custom profiles in the workspace `Cargo.toml`:
- `dev` (default): opt-level=2, debug assertions on — faster iteration than a true debug build.
- `release`: opt-level=3, full LTO, `panic=abort`.
- `no_overflow`: Used in world-gen crates to skip overflow checks for performance.

## 📋 Project Backlog (scored & prioritized)

**This is the master list of all pending work — the single always-present roadmap.** It is
intentionally **high-level (epics)**: it does NOT contain each spec/plan/task, instead every
record **references** the design docs (in the private `docs/design/` repo) that cover it. As we
build, MORE epics get added here (new mechanics, and content per class / race / weapon / monster /
vehicle / item). Keep this list current: when you finish or add work, update the row + score.

**Detail lives in the design repo** (`docs/design/`): specs `specs/`, plans `plans/`, task boards
`tasks/00-task-board.md` + `tasks/NN-*.md`, emerged-workstreams `2026-06-21-emerged-workstreams.md`.
The board `tasks/00-task-board.md` is the per-task source of truth; this backlog is the program-level
roll-up. Always read `docs/design/session-notes.md` + `agenda.md` on resume.

**Multi-session backlog — keep in sync.** This backlog is **shared and grows from multiple sessions**:
other sessions add `BL-NN` rows here (and design docs) as new mechanics / game needs surface. So
**`git pull` / re-sync `development` periodically** — at minimum before starting any new task and after
each merge — so you work against the current backlog and don't duplicate or collide. (Standard flow
already does `git fetch && git reset --hard origin/development` when starting new work after a merge;
do it on resume too.) When you add work, add the `BL-NN` row here **and** its detail in `docs/design/`,
then re-sort by score.

### Scoring rubric (so new items score consistently)
`Score = Value + Leverage + (6 − Effort)` → range 2–16, **higher = do sooner**.
- **Value (V) 1-5** — gameplay/project impact.
- **Leverage (L) 0-5** — how much it unblocks other work (foundational = high).
- **Effort (E) 1-5** — 1 ≈ days, 3 ≈ weeks, 5 ≈ months.
- **Status:** ✅ done · 🔵 in-progress · ⚪ pending · 🔒 blocked (dep) · 🟣 deferred.

### Backlog (sorted by priority score)

| ID | Epic / pending work | Area | V | L | E | Score | Status | Refs (docs/design) |
|----|---------------------|------|---|---|---|-------|--------|--------------------|
| BL-01 | **Per-class attribute structure + per-level scaling** (each class' HP/energy/stats profile; Mage energy-max grows with level → high-circle spells castable without nerfing costs) | Progression | 5 | 5 | 4 | **12** | ✅ | DONE (PR #63 + #64 cache). specs/2026-06-21-class-attributes-scaling; tasks/16. Pending: in-game smoke (BL-09); poise tier deferred; extend RON rows w/ BL-04 |
| BL-52 | **Combat resolution system — accuracy / miss / evasion / critical** (hybrid action+roll: keep Veloren's active dodge/block/precision AND add a probabilistic to-hit layer on hostile attacks; `hit% = clamp(base+(acc−eva)·k, 0.05, 1.00)`, floor 5% / ceil 100%; unified crit-chance reusing `precision_mult`; magic single-target rolls, AoE = auto-hit + passive resistance mitigation (roll-free for raids); allied heals always 100%; Fear→−accuracy). **Foundation the rest of the combat track depends on.** | Magic/Combat | 5 | 5 | 4 | **12** | ✅ | **DONE (code-complete) — was PRIORITY; paused BL-05 (Matías 2026-06-25), now resumed.** Decisions LOCKED (worksheet). specs/2026-06-25-combat-resolution-design; plans/2026-06-25-combat-resolution-plan; tasks/24. Supersedes PR #84's `attack_miss_chance` (Fear→−accuracy). Numbers in RON (extend class_attributes + combat-tuning asset). Per-tick stats (no DB migration). Sibling BL-53 later feeds acc/eva/crit. Unblocks BL-05 + all future hit/miss/crit/resist mechanics. **✅ CODE-COMPLETE 2026-06-26 (P1–P6 merged: #86 to-hit, #87 crit, #88 magic to-hit, #89 AoE resistances, #90 Miss feedback, #91 armor→evasion+shields, #98 P6 crit-damage floor). P6 balance review = no further RON tweaks pre-playtest (balance/2026-06-26-...-p6-integrated-review). **Remaining = in-game smoke only (BL-09, checklist docs/design/2026-06-26-bl52-bl05-smoke-checklist) → tune RON from feedback; pre-staged levers tabled.** Deferred polish: CR4.3 (crit word-label + miss sfx), resistance→debuff-duration, weapon-accuracy source, single protection-scan perf |
| BL-02 | **Content factory**: harden (tests/render_ron) → pilot → scale (Workflow) → install in `.claude/` | Tooling | 4 | 4 | 3 | **11** | 🔵 | specs content-factory-design; tasks/12. **Spell sweep DONE (2026-06-23): 577 spell sheets across all 11 schools** (`tools/content-factory/sheets/<school>/*.sheet.json`) — source of truth for `classes` (verbatim from JSON), `variant`, magic_source, and deferred riders (`-> file 13` notes). Validator/canon-lint clean. **Remaining = integration session (point 1): build `render_ron.py` → manifest/i18n/tests/PR** (content-adaptation-design Ph.1/2; needs balance numbers) |
| BL-03 | **Difficult-terrain mechanic** (persistent zone = half move-speed + immunity by race/item/spell; reusable for spells/terrain/weather) | Magic/World | 3 | 4 | 2 | **11** | ✅ | DONE (PR #67): `DifficultTerrain` + `FreedomOfMovement` BuffKinds; Magnify Gravity → slow-zone aura. specs/2026-06-22-difficult-terrain. Pending: Dark Star zone (→BL-05), Ranger immunity grant (→BL-04), in-game smoke (BL-09). Shares zone infra w/ BL-36 |
| BL-04 | **Classes-wave**: 10 new `ClassKind` (Barbarian/Sorcerer/Warlock/Bard/Paladin/Druid/Ranger/Monk/Artificer/**BloodSlayer**) — all 14 selectable + persistence + identity + empty trees | Classes | 5 | 5 | 5 | **11** | ✅ | DONE (PR #73): 14 classes selectable end-to-end (Mystic=source not class). specs/2026-06-22-classes-wave; tasks/17. Pending: populate trees (BL-06), Hemomancy re-gate→[BloodSlayer,Warlock]≤c5 (**spell session DONE 2026-06-23 → re-gate now unblocked, see BL-11**), bespoke outfits/implements + M2 (BL-17) |
| BL-05 | **Deferred spell riders** (forced-move, restrain, shared-fate, reaction/banish, random-table, prone, rapid-aging, melee-drain, multi-tick AoE, reflect, conditional-detonate, stun, anti-tp, blind/deafen, bleed-mark). ⏸️ **Wait for the spell-mapping task to finish** (new spells may introduce new mechanics). **On start (before coding):** read ALL rendered spells, catalog every distinct mechanic, discard identical ones, keep similar-but-distinct variants (variety is wanted), consolidate, and **append the resulting mechanic list to this BL-05 scope**. Engine half (mechanics) is lore-independent; content half re-points the rendered RONs (collision risk with the spell session → sequence after it). | Magic | 4 | 4 | 4 | **10** | 🔵 | specs/2026-06-24-spell-riders-engine; emerged WS-6; tasks/13. **Catalog DONE (2026-06-24): 34 distinct mechanics** (11 exist / 11 partial / 12 new). **v1 slice locked** (worksheet): A Charm+Fear (min AI, rich AI→AURORA) · B Smite/Sleep/Anchor/TempHP · C ForcedDisplacement/Prone/Blind/bleed-detonate. **Batch 1 in PR: `Anchored`+`Asleep`.** Defers: random-table→BL-43, counterspell→BL-26, anchor-utils→BL-27, summon-ctrl→BL-37, element-select→BL-02; invis/telekinesis/telepathy/resurrect→**BL-51 (wanted)**. Content half (re-point RONs) waits BL-02; balance pass TODO. **UNBLOCKED 2026-06-23: spell-mapping done (577 sheets, 11 schools).** The deferred-rider inventory already lives in the sheets' `-> file 13` notes — catalog from those (or from rendered RONs once BL-02 integration runs). Content half (re-point RONs) still sequences after render (BL-02 point 1); engine half (mechanics) can start now. **▶ RESUMED 2026-06-25 — BL-52 is code-complete (#86–#91), Fear now ships as `−accuracy` (PR #84 closed/superseded).** **ENGINE SLICE ✅ DONE 2026-06-26:** Anchored/Asleep (#81), Blinded/sleep-wake (#82), Charm/Fear AI (#83), TempHP (#93 mechanic + #95 HUD bar), bleed-detonate (#97). **Remaining BL-05 = content-half only** — Smite/ForcedDisplacement/Prone reuse existing primitives, need RONs re-pointed → **gated on BL-02** (owner TBD). **Adopt the condition→±accuracy/evasion mappings** (Blinded/Restrained/Prone/Invisible) from specs/2026-06-25-content-combat-mapping instead of inventing per-rider misses |
| BL-06 | **Populate class skill trees** (per-class skills: passive stat boosts + signature ability unlocks). **v1 = ALL 14 classes**, built incrementally — proof slice Warrior/Mage/Cleric/Rogue (melee/magic/heal/ranged) first, then extend. **Skills only** (starter kits → BL-57). | Classes | 4 | 3 | 4 | **9** | 🔵 | **Structural decisions LOCKED (worksheet 2026-06-26):** Q1=C passives+abilities, Q2=A hand-laid diary trees (reuse existing icons; bespoke art later), Q3=B ~10-12/class **research-backed** (D&D 5e + ARPG/MMO), Q4 no kits. specs/2026-06-26-class-skill-trees-design; plans/...; tasks/26. `SkillGroupKind::Class` exists but empty → add `Skill` class enums + per-skill effects (passives reuse BL-01/BL-52 stat fields; abilities gate on `has_skill`) + 3 manifests + diary UI tab. **NEXT = P0 research + per-class content worksheet** before code. (E bumped 3→4: scope now all 14 + abilities + UI.) |
| BL-53 | **Ability scores (STR/DEX/CON/INT/WIS/CHA)** (persisted attribute layer modified by race/class/level/weapons/armor/buffs/debuffs; feeds derived stats — accuracy/evasion/crit, HP/energy/damage, carry, spell power, resistances). Sibling of BL-52: ships after it, then plugs in as an added source w/o changing resolution math. **Investigation-first** (research + worksheet before code). | Progression | 4 | 5 | 5 | **10** | ⚪ | specs/2026-06-25-ability-scores-design (INVESTIGATION, §5 worksheet pending); plans/2026-06-25-ability-scores-plan; tasks/25. Persisted comp → DB migration (mirror Ethos V72). Extends BL-01/BL-04/races; relates BL-48. Study D&D 5e `(score−10)/2` + WoW/Diablo MMO stat models. Guard double-count w/ BL-52 armor-weight evasion |
| BL-32 | **Player parties (12 now → 25 later)** (raise group cap; key for RIDE events + Battle PITS) | Social | 4 | 3 | 3 | **10** | ✅ | DONE: `max_player_group_size` default 6→**12** (interim per Matías; bump to 25 once engine/server proven). Admin-tunable; group sys + HUD scale dynamically. specs/2026-06-22-parties-25. Pending: HUD polish at large parties, sync bandwidth, in-game smoke. Unblocks BL-42 |
| BL-07 | **Item content render** (1.825 items → `ItemKind` RON; flat-stat cores first) | Content | 5 | 3 | 5 | **9** | 🔒 | tasks/11,12 (blocked on CA-P0 decisions) |
| BL-47 | **Multi-coin currency + economy revaluation** (platinum/gold/silver/copper at 1:100, copper base/common; revalue all prices, loot, merchant stock into one stable, easy-to-understand system) | Economy | 4 | 3 | 4 | **9** | ⚪ | specs/2026-06-24-currency-revaluation; tasks/19. Research done (D&D 1:10 physical / WoW 1:100 unified-balance / Veloren coin-item 1:1) + full code map (coin=item today, ~245 loot tables, trade_pricing/economy/rtsim, HUD). Locked ratios 1pp=100g=100·100s=100·100·100c. Decisions pending (worksheet): money model (A unified copper balance [rec] / B 4 items / C hybrid), revaluation approach, display, conversion, spread, loot strategy. Phases: P0 decisions+balance tables, P1 plumbing, P2 revaluation pass; supply/demand → AURORA (BL-15). Coordinate w/ BL-07/BL-22 item values; restate spell-transcription cost |
| BL-08 | **Spellbook/compendium UI consumer** + tool-slot wiring (`ability_set_manifest`) + i18n backfill (3 old v2 spells) + HUD cooldown/attune-progress bars | Magic/UI | 3 | 2 | 3 | **8** | ⚪ | comp/spell.rs; tasks/13 |
| BL-09 | **In-game verification checks** (need Matías in client: magic-v2 casters, CLS-11 `/set_class`, EQ-B8 tooltip, attunement, new spells) | QA | 3 | 0 | 1 | **8** | ⚪ | tasks/00 "Pending in-game checks" |
| BL-49 | **Xindeler world map — Highlands adaptation + plane-map plan** (program-scale: adapt Veloren's procedural world into the canon **Highlands** continent — coastline/mountains/rivers/lakes, the 4 parts (Merovingia/Cromatolis/Xandrian/Freelands), pinned cities/towns/villages, swamps, denser caves w/ all biomes — then plan (not build) the other planes' maps. **V1 = Highlands only**, V2+ one continent/plane at a time) | World | 5 | 2 | 5 | **8** | ⚪ | specs/2026-06-24-xindeler-worldmap; plans/2026-06-24-xindeler-worldmap-plan; tasks/21. **Created skill `xindeler-worldmap` + agent `worldmap-cartographer`.** Key lever = **heightmap import** (`world/src/sim` `FileOpts`/`WorldMap_0_7_0` .bin → terrain shape; rivers/biomes/sites/caves derive from it); sites need a new additive **authored-site-pin** in `civ/`; biomes biasable (re-enable Swamp); caves tunable (`layer/cave.rs`). Grounded in lore/80-geography + 40-planes. Decisions pending (worksheet): world size, heightmap source/tool, pin granularity, biome mechanism, cave target, planes-V1 approach. Phases P0 tooling/cartography → P1 heightmap → P2 site-pins → P3 biomes/swamps/caves → P4 map asset → P5 plane-map plan. Keep `world/` additive/config-gated (upstream churn); binary maps → VPS-LFS. Foundational for BL-13 (zones), AURORA/ORACLE (geography) |
| BL-50 | **Engine evaluation — current vs Bevy/Unity/Unreal** (evaluation epic: document our bespoke Veloren engine (wgpu/specs/winit/greedy-voxel; ~395k LOC) + pros/cons, then compare "improve vs replace" for the graphics goal — better-defined cubes + world detail, keep the voxel/cubist look. **Decision gated**; choosing any path mandates a separate full migration spec/plan/tasks) | Engine/Strategy | 3 | 2 | 3 | **8** | ⚪ | specs/2026-06-24-engine-evaluation; plans/2026-06-24-engine-evaluation-plan; tasks/22. Dossier done. Paths: **A** upgrade our renderer (PBR/normal-maps/GI/higher-def cubes — contained to `voxygen/render/`+shaders, ~1-2mo, serves the goal cheapest) · **B** Bevy (rewrite client, keep Rust server/world/logic ~206k LOC, ~3-4mo, only migration that stays Rust+wgpu) · **C** Unity / **D** Unreal (full client rewrite, no native Rust, voxel-from-scratch + Rust↔engine bridge, 4-7mo+). Server/worldgen/sim survive in all. ⚠️ migrating off Veloren ends upstream-sync. Work = deep-dives + **spikes** (A graphics POC + B Bevy POC) → scored matrix → decision worksheet → (conditional) migration program. Ties to BL-49 (visual detail) |
| BL-33 | **Alignment system** (Good/Neutral/Evil × Lawful/Neutral/Chaotic for PCs & NPCs; enriches underworld/factions, feeds AURORA) | Systems/Sim | 3 | 3 | 4 | **8** | ✅ | `comp::Ethos` (9-box scores; distinct from AI `comp::Alignment`). DONE end-to-end: P1 sync + `/set_ethos` (#75); P2a PC persistence migration V72 (#76); P2b NPC assignment (#77); P3 PC deed-drift on kills (#78); creation-pick UI (#79, in-client smoke ✓ — 5-step wizard w/ Alignment step + Review recap). specs/2026-06-22-alignment-system; tasks/18. AURORA-era follow-ups (behavioural consequences + NPC drift + more deeds) feed BL-15 |
| BL-48 | **Magic-item sockets + socketing craft** (sockets by quality — Common 0/Uncommon 1/Rare+VeryRare 2/Legendary 3/Mythic 4; insert gems/magic-items for buffs or a new quality (spell/mechanic); socketing = a craft with a forgiving failure% gated by a new use-grown **"magic item knowledge"** proficiency that rises by investigating items) | Items/Crafting | 4 | 2 | 5 | **7** | ⚪ | specs/2026-06-24-magic-item-sockets; tasks/20. Research (D2/PoE sockets+runewords, D&D attunement) + code map: sockets = new per-instance `Item` field (persist via `DatabaseItemProperties` like durability), **reuse the existing attunement effect-gating** (don't extend modular components), new `SocketingEvent` w/ success roll, knowledge = new use-grown synced+persisted proficiency. Decisions pending (worksheet): slot table (Epic 2 vs 3), hosts (weapons/armor/spellbooks), effect tiers, failure consequence, knowledge growth/gating, unsocketing, runewords-defer. Phases P0 decisions+curves → P1 socket model+persist → P2 craft+buff-gems → P3 knowledge → P4 spell-grant inserts. Ties to BL-07/BL-22 (gems), BL-47 (gem values), BL-08/BL-19 (spellbooks) |
| BL-11 | **Blood Slayer depth** (the `BloodSlayer` ClassKind now exists via BL-04; remaining: re-gate Hemomancy `[Mage]`→`[BloodSlayer,Warlock]` ≤circle-5 + full blood-rite kit/skills) | Classes | 3 | 2 | 3 | **8** | ⚪ | emerged WS-5; tasks/17 P4. **UNBLOCKED 2026-06-23: spell session done — the Hemomancy sheets already carry `classes: [Blood Slayer, Warlock, Mage, Sorcerer]` with Blood Slayer capped ≤ circle 5** (sheets are the source of truth; re-gate = render those into compendium during BL-02 integration) |
| BL-12 | **Race passives → the 6 playable species** (mine traits; no new bodies) | Races | 2 | 2 | 3 | **7** | ⚪ | tasks/11 §5.2 |
| BL-13 | **World difficulty zones** (level bands, NPC class mapping; planes Phase-4 deferred) | World | 3 | 2 | 4 | **7** | ⚪ | tasks/05 |
| BL-14 | **IP depuration remaining** (arcanist leaves + roster + scrub ~28 tokens → denylist) | Lore/IP | 2 | 2 | 3 | **7** | 🔵 | tasks/14 |
| BL-15 | **Project AURORA** (NPC social sim: relationships, memory, families, orgs, economy, dynamic quests — 35 tasks) + **generative-NPC layer** (LLM/voice/persona automation) | Sim | 4 | 2 | 5 | **7** | ⚪ | specs/2026-06-10-project-aurora; tasks/08. **Generative-NPC companion** (2026-06-24): spec `2026-06-24-aurora-generative-npc-design`; tasks/23; **skill `xindeler-ai-npc` + agent `npc-persona-writer`**. Two-tier: T1 **offline pipeline** (Claude + ElevenLabs-MCP pre-bake personas/dialogue/voice → game data; runtime zero external deps; honors "no LLM in tick path") · T2 **optional live** (self-host faster-whisper+LLaMA+fast-TTS on VPS). Tools: ElevenLabs offline / **NOT PlayHT** (Meta-deprecated) / faster-whisper self-host / defer Audio2Face (voxel faces). Prod refs: NVIDIA ACE, Inworld, Convai, OpenAI Realtime |
| BL-16 | **Project ORACLE** (world director: events, story arcs, monster ecosystem, climate, narrative) | Sim | 4 | 2 | 5 | **7** | ⚪ | specs/2026-06-10-project-oracle; tasks/09. **Design addendum (2026-06-24)** `2026-06-24-oracle-design-addendum` closes gaps vs the full World-Director GDD: politics/diplomacy (faction goals, treaties/embargoes/marriages), **macro-economy + explicit ORACLE↔AURORA seam** (ties BL-47), **Quest-Opportunity Generator** (formal ORACLE→AURORA quest handoff w/ BL-15), perception/event-intake pipeline, causal-graph + historical query API, religion/culture spread + `development` scalar (tech substitute), event-scale tag, distributed/sharding = post-v1, 8↔11 phase map (+2 phases, ~130 dev-days). Skills `xindeler-oracle`/`xindeler-aurora` exist; added review agent **`sim-design-reviewer`** (design) alongside `sim-systems-engineer` (impl). Builds on BL-15 (AURORA) |
| BL-17 | **Refactor M2** (class→starting-weapon whitelist → single source in `comp/class.rs`) | Cleanup | 1 | 1 | 1 | **7** | ⚪ | tasks/00 backlog; PR #22 |
| BL-18 | **Attunement persistence** (session-only → DB migration) | Items | 3 | 1 | 3 | **7** | 🟣 | tasks/13. **678 source items** attune (461 any + 217 conditional → add a requirement-predicate via `ItemRequirements` class/race/level/`Ethos`). specs/2026-06-24-item-mechanics-catalog |
| BL-19 | **Readable scroll/book `ItemKind` + spell transcription system** (circle↔level, arcane ink, gold/time) | Magic/Items | 3 | 2 | 4 | **7** | ⚪ | tasks/13; specs spell-transcription |
| BL-34 | **Username + character-name validation** (server-side anti-offensive filter) + character-name **uniqueness** | Server/Moderation | 3 | 1 | 3 | **7** | ⚪ | backlog-additions §BL-34 |
| BL-35 | **Xindeler Admin Panel** (web, mobile-first: `xindeler-manage` deploy/start/stop/logs/ban/warn/broadcast/email + AURORA/ORACLE control + `xindeler-health-check` port 14004) | Infra/Web | 4 | 2 | 5 | **7** | ⚪ | backlog-additions §BL-35; admin-guide.md (AI parts dep BL-15/16) |
| BL-36 | **Antimagic fields / spells** (zone suppresses casting + nullifies magic items → mundane) | Magic | 3 | 2 | 4 | **7** | ✅ | DONE (PR #69): `Antimagic` BuffKind + `DisableMagic`; magic-only cast gate; attuned item effects mundane; `Antimagic Field` spell. specs/2026-06-22-antimagic-field. Pending: tool-slot wiring (BL-08), full stat-nullification (phase 2), zone shapes (cone/cylinder/dome) |
| BL-37 | **Sidekicks** (mercenary/honor NPC allies; AI + obey orders unless suicidal; party ≤6) | NPC/Sim | 4 | 2 | 5 | **7** | ⚪ | backlog-additions §BL-37 (dep BL-15) |
| BL-38 | **Consumable restrictions** (by race/class/level via `ItemRequirements`) | Items | 2 | 1 | 2 | **7** | ⚪ | backlog-additions §BL-38; eq-restrictions PR #24 |
| BL-39 | **Bug-report system re-apply** (VPS changes) + rename → `xindeler-bug-report` | Infra | 2 | 0 | 1 | **7** | ⚪ | backlog-additions §BL-39 |
| BL-20 | **Feats / optionalfeature → class skills** (invocations/maneuvers/metamagic/infusions/…) | Classes | 2 | 2 | 4 | **6** | 🔒 | tasks/11 §5.2 (dep classes-wave) |
| BL-21 | **Lore Canon Wave D residuals + open set-pieces** + rewrite the stale `06` board | Lore | 2 | 1 | 3 | **6** | ⚪ | tasks/06; session-notes |
| BL-22 | **Weapons / armor / consumables content render** (file-11 waves) | Content | 3 | 1 | 4 | **6** | 🔒 | tasks/11 (dep CA-P0) |
| BL-23 | **Magic-v2 P4 residuals** (M1 Innate index→key persistence migration; P4.15) | Magic | 2 | 1 | 3 | **6** | 🟣 | tasks/04 |
| BL-24 | **ENG-D3 charges + ENG-D4 wondrous spell-attach** (item mechanics) | Items | 2 | 1 | 3 | **6** | ⚪ | tasks/13. **Demand (2026-06-24):** ENG-D3 charges = **210 items** (charge pool + recharge clock dawn/dusk/midnight + dice + expend-destroy); ENG-D4 spell-attach = **311 items** (bind compendium spell + gate at-will/daily/per-rest/charges/ritual). Sequence D3→D4. specs/2026-06-24-item-mechanics-catalog |
| BL-46 | **New weapon types (`ToolKind`)** (Mace/Whip/Sling/Firearm/Trident/Flail/Sickle/Morningstar/War-Pick; enum + ability sets + anims/icons + skill-tree; sheets keep the real type meanwhile) | Weapons/Engine | 2 | 2 | 4 | **6** | ⚪ | emerged WS-7; imports/missing-weapon-types |
| BL-25 | **Engine improvements remaining** (tracy cells; ENG-5 captures; ENG-8/9 phase gate) | Engine | 2 | 1 | 3 | **6** | 🔵 | tasks/07 |
| BL-40 | **Rename `veloren-*` → `xindeler-*`** (crates/bins/refs; NOT assets) ⚠️ raises upstream-merge conflict surface. Explore a `veloren→xindeler` **mapping script** applied automatically during each pull/merge (custom merge-driver / rename-on-a-raw-branch / sed-fastmod) so **"Veloren" disappears from Xindeler's code without breaking upstream-sync** | Infra/Cleanup | 2 | 1 | 3 | **6** | ⚪ | backlog-additions §BL-40 |
| BL-41 | **Elves have no beard** in PC creation (hide beard option for elf) | Client/UI | 1 | 0 | 1 | **6** | ⚪ | backlog-additions §BL-41 |
| BL-42 | **Battle PITS** (dedicated PvP arenas 1v1 / 2v2 / 3v3 / 6v6 / 12v12 / 25v25) | PvP/World | 4 | 1 | 5 | **6** | ⚪ | backlog-additions §BL-42 (dep BL-32) |
| BL-51 | **Advanced spell subsystems** (invisibility/stealth · telekinesis/object-control · telepathy/mind-message · resurrection/special-heal) — surfaced from the BL-05 rider catalog; **wanted for v1** (Matías), each a bigger subsystem needing its own sub-spec (stealth→AI-perception, telekinesis→object-manip, telepathy→chat/UI, resurrect→death/respawn) | Magic | 3 | 2 | 5 | **6** | ⚪ | specs/2026-06-24-spell-riders-engine §5; tasks/13. Split out of BL-05 (NOT deferred indefinitely). Invisibility overlaps AURORA perception; resurrection needs death-integration. Each → own spec/plan/tasks when scheduled |
| BL-26 | **Counterspell / dispel** (magic Phase E) | Magic | 2 | 1 | 4 | **5** | 🟣 | tasks/13 |
| BL-27 | **Axiomancy utility mechanics** (luck token, object-anchor, extradimensional item-stash) | Magic | 1 | 1 | 3 | **5** | ⚪ | emerged WS-4; tasks/13 |
| BL-28 | **Client release pipeline** (desktop packaging + Airshipper, self-hosted runner on VPS) | Infra | 3 | 0 | 4 | **5** | 🟣 | CLAUDE.md "Releases & CI" (defer → first client release) |
| BL-43 | **Deck of Many Things** (random-effect-table item) | Items/Magic | 2 | 1 | 4 | **5** | ⚪ | backlog-additions §BL-43 (dep BL-05) |
| BL-44 | **Animal companion** (attachment bar; spirit/magical by class/subclass) | NPC/Classes | 3 | 1 | 5 | **5** | ⚪ | backlog-additions §BL-44 (dep BL-37, BL-04) |
| BL-45 | **Mate easter egg** (serve & drink an Argentine mate) | Content | 1 | 0 | 2 | **5** | ⚪ | backlog-additions §BL-45 |
| BL-29 | **Optional rules adoptions** (Firearms / Fear & Horror / Hero Points / Injuries…) | Content | 1 | 0 | 3 | **4** | ⚪ | tasks/11 §6 |
| BL-30 | **Vehicles / mounts / ships system** (no system exists) | Systems | 2 | 0 | 5 | **3** | ⚪ | tasks/11 §3.2 (out-of-scope today) |
| BL-31 | **Backgrounds system** (no system exists) | Systems | 1 | 0 | 4 | **3** | ⚪ | tasks/11 §5.2 |
| BL-54 | **Cursed-item mechanic** (bind-on-equip/attune: can't unequip/un-attune until remove-curse; hidden-until-identified/triggered; penalty reuses `BuffKind::Cursed`) | Items | 2 | 1 | 2 | **6** | ⚪ | NEW 2026-06-24; **32 items**. Effect exists (`BuffKind::Cursed`); the item-binding+reveal mechanic does not. Reuses attunement (BL-18) + buff infra. specs/2026-06-24-item-mechanics-catalog |
| BL-55 | **Sentient items** (item persona: mental stats + `Ethos` alignment + comms speech/telepathy/empathy + senses + special-purpose + **ego-conflict** contest vs wielder) | Items/Sim | 3 | 1 | 5 | **5** | 🔒 | NEW 2026-06-24; **39 items** (Sunsword etc.). No engine support. A sentient item ≈ an NPC bonded to a slot → **build on BL-15 AURORA**; until then render as ordinary magic items, sentience dormant in lore. specs/2026-06-24-item-mechanics-catalog |
| BL-56 | **Element vulnerability** (take extra damage of a type — the deliberate counterpart to resistance; BL-52 floors negative resistance at 0, so this is opt-in content) | Magic/Combat | 1 | 1 | 2 | **4** | ⚪ | NEW 2026-06-25; optional. Extends BL-52 typed-resistance; see specs/2026-06-25-content-combat-mapping |
| BL-57 | **Starter Packs / class kit grants** (per-class starting equipment — weapon/implement/outfit/consumables — via a char-creation script or a "Starter Packs" manifest, NOT tied to the skill-tree logic) | Classes/Items | 3 | 1 | 2 | **8** | ⚪ | NEW 2026-06-26 (split out of BL-06 Q4, Matías): keep progression (BL-06 skills) separate from initial inventory/loot. classes-wave (BL-04) already gives outfits; this adds the starting weapon/implement + kit per class, cleanly in a manifest. Do after BL-06 |
| BL-58 | **VPS LFS integrity recovery + post-smoke sync** (recover binaries the last upstream sync committed as pointers but never pushed to the VPS, + bundle BL-06/52/05 smoke-report fixes into one sync pass) | Infra/Assets | 2 | 1 | 2 | **7** | ⚪ | NEW 2026-06-26 (Matías). Found during P4 smoke setup: `assets/voxygen/audio/ambience/river_loud.ogg` + `river_quiet.ogg` are LFS pointers with **no blob on the VPS** (`git lfs fetch` returns nothing) — currently substituted LOCALLY with `rain.ogg` placeholders (uncommitted, do not commit). Recover the real binaries (from `gitlab` upstream / regenerate) + `git lfs push` to the VPS; audit the sync for any other un-pushed blobs; harden `upstream-sync` to verify LFS-push integrity. Do AFTER the smoke report so any RON/code tweaks ride the same sync. |
| BL-59 | **BL-06 diary UX polish** (skill NAME above each tile + spacing; show prerequisites to unlock; varied/less-repeated icons + more "epic" feel; Spanish i18n for class strings + "Class"/"Class Skills") | Classes/UI | 2 | 1 | 2 | **7** | ⚪ | NEW 2026-06-27 (P4 smoke). Pure polish on the generic class-tree renderer; separate from the P4 BUGS (active-tile broken icon/i18n, rank-not-updating) which fix immediately. Findings: docs/design/2026-06-27-p4-smoke-findings.md |
| BL-60 | **NPC status indicators** (show active buff/debuff/cooldown icons above NPC heads so players SEE CC/effects — fear/sleep/charm/blind/bleed/shield, etc.) | UI/UX | 3 | 2 | 3 | **8** | ⚪ | NEW 2026-06-27 (Matías' G2 top priority). Makes BL-05 CC legible in play; prerequisite for meaningfully testing/feeling crowd-control. Findings doc 2026-06-27-p4-smoke |
| BL-61 | **Out-of-combat regen + regen skill** (slow HP/energy regen when not in combat / out of NPC aggro; a class/general skill raises rate+amount with conservative values) | Progression/Combat | 3 | 1 | 3 | **7** | ⚪ | NEW 2026-06-27 (P4 smoke A3). Pairs with the max-HP-clamp bugfix; gives leveled HP a reason to matter |
| BL-62 | **Combat feel: melee hit forgiveness + crit feedback** (hit within a small radius even if not perfectly facing, reach scaled by weapon; "Critical Hit" floating label + number; minor crit-damage tune) | Combat | 3 | 1 | 3 | **7** | ⚪ | NEW 2026-06-27 (P4 smoke B1/B2). Crit-label part = BL-52 CR4.3 (already deferred). Hit-radius is a melee-cone tuning, independent of the BL-52 to-hit roll |
| BL-63 | **CC/buff test affordance** (apply buffs to a TARGETED NPC via admin since `/buff` is self-only, + documented undead-spawn; needed because BL-05 CC has no player-facing application until the spell content lands) | Infra/QA | 2 | 1 | 1 | **7** | ⚪ | NEW 2026-06-27 (P4 smoke C*/A7). Small: extend `/buff` with an optional EntityTarget, or a `/target_buff`. Unblocks in-game CC verification before BL-02 spell content |
| BL-64 | **Playable race expansion + renames** (rename in-world race names; add subraces per lineage; add NEW playable races) | Races | 3 | 2 | 5 | **6** | ⚪ | NEW 2026-06-27 (Matías). **3 effort tiers:** (a) **renames = cheap/i18n-only** — display names come from `common-species-*` (en/es); rename Danari→Gnome/Gnomo, Draugr→Threshold-walker; also fix char-creation summary `ui.rs:1848` which uses `{:?}` (Debug enum) instead of the i18n name. (b) **subraces = medium** — `Species` has no subrace concept; options: cosmetic variants (extra skin/hair/feature options + a subrace tag) / new species each / a passive mechanic (ties BL-12 race passives) — design choice. (c) **new races (Tiefling/Fiend-Marked, Halfling/Smallfolk) = MAJOR art** — new `humanoid::Species` variant (enum has explicit discriminants 0-5) needs bespoke voxel head/hair/beard/accessory/eyes models per body type + skin/hair/eye color sets + animations + i18n. Seed table (Matías): Human(Standard/Variant), Dwarf(Hill/Mountain), Elf(High/Wood/Drow), Orc(Fullblood/Half), Draugr=Threshold-walker(Revenant/Bloodbound), Gnome/Danari(Forest/Rock), Tiefling=Fiend-Marked, Halfling=Smallfolk(Lightfoot/Stout). Needs a design worksheet before content. **Renames DONE (#114): Danari→Gnome/Gnomo, Draugr→Dhampir/Dhampiro.** Remaining = subraces (b) + new races (c). |
| BL-65 | **NPC combat stats — accuracy/evasion/crit baseline** (NPCs have no to-hit stats, so BL-52's hit/miss/evasion is one-sided & invisible in PvE) | Combat/Balance | 3 | 2 | 3 | **8** | ⚪ | NEW 2026-06-27 (P4 smoke B1 root cause). Confirmed: `stats.accuracy`/`evasion` are written ONLY by `ClassAttributes::apply` (gated to humanoid PCs w/ a class), class skills, and buffs — **mobs default to 0/0**. Result: a leveled player's accuracy clamps `hit% = base_hit + acc·hit_k` to the 1.0 ceil by ~lvl 10 vs 0-evasion mobs → **never misses**; and caster AoE/beam auto-hits (no roll) so mobs rarely miss either → the "Miss" SCT (which works) almost never shows. Fix: give NPCs an accuracy/evasion (and crit) baseline scaled by body/level/difficulty (mirror `base_health`; values via game-balance-designer), so the to-hit math is two-sided. Ties BL-13 (difficulty zones / NPC level) + BL-52. Interim option: a flat NPC evasion baseline to make misses visible. |

**🟣 Deferred to v2 (not scheduled):** Terrain resolution / smooth-terrain — see `docs/design/DEFERRED-TO-V2.md`.

> **Growth:** new content (each class, race, weapon, monster, vehicle, item) and each new mechanic
> gets a new `BL-NN` row here, scored with the rubric, with its specs/plans/tasks created in
> `docs/design/` and referenced in the Refs column.

### Dependencies & parallel tracks

**Dependency edges (X → Y = X needs Y first):**
- BL-04 (classes-wave) → BL-01 (per-class attributes). BL-11 (Blood Slayer) → BL-01 + BL-04.
  BL-44 (animal companion) → BL-04 (subclasses) + BL-37. BL-20 (feats) → BL-04.
- BL-07 / BL-22 (content render) → BL-02 (content factory). 
- BL-36 (antimagic field) shares the **persistent-zone** infra with BL-03 (difficult-terrain) — do BL-03 first.
- BL-43 (Deck of Many Things) → BL-05 (random-effect-table rider).
- **BL-05 (spell riders) → BL-52 (combat resolution)** — ✅ satisfied: BL-52 is code-complete (#86–#91),
  BL-05 **resumed** (Fear ships as −accuracy; PR #84 superseded). BL-53 (ability scores) is a sibling of
  BL-52: ships after it, then feeds accuracy/evasion/crit as an added source (no resolution-math change);
  extends BL-01/BL-04/races.
- BL-08 / BL-10 / BL-23 → the magic engine already merged; BL-26 (counterspell) is independent magic.
- BL-16 (ORACLE) builds on BL-15 (AURORA). BL-33 (alignment), BL-37 (sidekicks), and BL-35's AI
  section all feed/await **BL-15 AURORA** (+ BL-16 for ORACLE-review).
- BL-42 (Battle PITS) → BL-32 (parties). 
- BL-40 (rename) is coupled to the **upstream-sync** cycle — run it right after a sync, scripted.

**Parallel tracks (independent; can advance concurrently):**
- **A · Progression/Classes:** BL-01 → BL-04 → {BL-11, BL-06, BL-20, BL-44}.
- **B · Magic/Combat mechanics:** **BL-52 (combat resolution) ✅ code-complete → BL-05 (active, resumed)** → BL-43; BL-53 (ability scores) sibling of BL-52; BL-03 → BL-36; plus BL-10, BL-18, BL-19, BL-24, BL-26, BL-46 (new weapon ToolKinds) (mostly independent of A).
- **C · Content & tooling:** BL-02 → {BL-07, BL-22}; BL-08.
- **D · Simulation:** BL-15 (AURORA) → BL-16 (ORACLE) → integrate BL-33, BL-37.
- **E · Social/PvP:** BL-32 → BL-42.
- **F · Infra/Ops:** BL-35 (server-mgmt + health-check parts startable now; AI parts after D), BL-39, BL-40, BL-28, BL-25.
- **G · Quick wins (parallel anytime, low effort):** BL-09, BL-17, BL-38, BL-41, BL-45, BL-21, BL-12.

**Suggested starting set (high score, no blockers, parallelizable):** BL-01 (track A), BL-03 (track B),
BL-02 (track C), BL-32 (track E), plus quick wins BL-41 / BL-38 / BL-09. AURORA (BL-15) is the gate for
most of the Sim/NPC work, so starting it early unblocks BL-16/33/37/44 and the admin-panel AI section.
