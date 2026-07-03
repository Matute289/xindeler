#!/bin/bash
# cargo clippy is a superset of cargo check,
# so we don't check manually.

# BL-82 (EM-1.2): the engine-isolation law is checked FIRST (cheap, metadata-only):
# logic crates must never depend on bevy/wgpu/winit or on any bevy/* shell crate.
"$(dirname "$0")/../../scripts/check-engine-isolation.sh" &&

time cargo clippy \
    --all-targets \
    --locked \
    --features="bin_compression,bin_csv,bin_graphviz,bin_bot,bin_asset_migrate,bin,stat,cli" \
    -- -D warnings &&

# BL-82 (EM-1.1): the `veloren-voxygen default-publish` clippy block was removed —
# voxygen is no longer a workspace member (legacy client kept in-tree as an
# unbuilt reference; reference builds live in xindeler-old).

# Ensure that test-server compiles.
time cargo clippy --locked --bin xindeler-server-cli --no-default-features -F simd  -- -D warnings &&
time cargo fmt --all -- --check;
