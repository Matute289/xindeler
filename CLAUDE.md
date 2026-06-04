# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

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

## Hot-reloading

In dev builds, `voxygen-anim` and `server-agent` are compiled as `cdylib` crates and loaded at runtime. Changes to animation or AI code reload without restarting. This is gated by the `hot-reloading` feature; the `default-publish` feature set disables it for release builds.

## Features of Note

- `tracy` — Enables Tracy profiler integration across crates.
- `asset_tweak` — Allows runtime asset value tweaking for balancing.
- `simd` — Enables SIMD optimizations in server-cli.
- `bin_*` — Various utility binaries (CSV export, graph generation, bot, asset migration).

## Build Profiles

Custom profiles in the workspace `Cargo.toml`:
- `dev` (default): opt-level=2, debug assertions on — faster iteration than a true debug build.
- `release`: opt-level=3, full LTO, `panic=abort`.
- `no_overflow`: Used in world-gen crates to skip overflow checks for performance.
