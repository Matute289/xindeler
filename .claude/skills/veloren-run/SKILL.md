---
name: veloren-run
description: Use when launching the Veloren game client or server — knows the correct env vars, features, hot-reload behavior, and cargo aliases
---

# veloren-run

## Pre-requisites

Always ensure the Rust toolchain is on PATH before running:
```bash
source "$HOME/.cargo/env"
```

`VELOREN_ASSETS` must point to the assets directory. In the project root:
```bash
export VELOREN_ASSETS="$(pwd)/assets"
```

## Launching the Client (GUI)

**Standard dev build** (hot-reloading enabled for animations and UI):
```bash
cargo run --bin veloren-voxygen
```

**Without hot-reloading** (faster startup, closer to release):
```bash
cargo test-voxygen
```

**With Tracy profiler** (performance profiling):
```bash
cargo tracy-voxygen
```

**With debug symbols** (for lldb/gdb):
```bash
cargo dbg-voxygen
```

**Release build** (no hot-reloading, LTO, for shipping):
```bash
cargo build --release --no-default-features --features default-publish
./target/release/veloren-voxygen
```

## Launching the Server (Headless)

**Standard dev server** (with worldgen, hot-reload agent AI):
```bash
cargo server
# equivalent to: cargo run --bin veloren-server-cli
```

**Minimal server** (no hot-reloading, faster startup for testing net code):
```bash
cargo test-server
# equivalent to: cargo run --bin veloren-server-cli --no-default-features --features simd
```

**With Tracy profiler**:
```bash
cargo tracy-server
```

## Single-Player (Client embeds Server)

Single-player mode is built into `veloren-voxygen` via the `singleplayer` feature (on by default in dev). Just run the client — no separate server process needed:
```bash
cargo run --bin veloren-voxygen
# then select "Singleplayer" in the main menu
```

## Hot-Reloading Behavior

In dev builds, two crates are compiled as dynamic libraries and reloaded at runtime without restarting:
- `server-agent` — NPC AI behavior (gated by `hot-agent` feature)
- `voxygen-anim` — character animations (gated by `hot-anim` feature)

To pick up changes: save the file → the game reloads it automatically within a few seconds.

Hot-reloading is **disabled** in `default-publish` (release) builds.

## Configuration Files

- Server settings: `userdata/server/settings.ron` (created on first run, gitignored)
- Client settings: `userdata/client/settings.ron` (created on first run, gitignored)
- Cargo aliases: `.cargo/config.toml`

## Troubleshooting

- **"assets not found"**: Make sure `VELOREN_ASSETS="$(pwd)/assets"` is set and you're running from the project root.
- **Compilation errors on first run**: The project requires nightly Rust. Run `rustup show active-toolchain` — it should say `nightly-2025-09-08`.
- **Window doesn't appear**: Check macOS permissions (accessibility, network). Try running from terminal directly.