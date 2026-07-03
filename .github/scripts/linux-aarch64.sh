#!/bin/bash
# LEGACY (BL-82): unreferenced by our workflows; still packages the voxygen binary,
# which this workspace no longer builds. Kept only as an upstream reference.
export VELOREN_USERDATA_STRATEGY=executable;
export PKG_CONFIG="/usr/bin/aarch64-linux-gnu-pkg-config";
time cargo build --target=aarch64-unknown-linux-gnu --release --no-default-features --features default-publish;

aarch64-linux-gnu-objcopy --compress-debug-sections=zlib \
    target/aarch64-unknown-linux-gnu/release/xindeler-server-cli \
    target/aarch64-unknown-linux-gnu/release/xindeler-server-cli-compressed
aarch64-linux-gnu-objcopy --compress-debug-sections=zlib \
    target/aarch64-unknown-linux-gnu/release/veloren-voxygen \
    target/aarch64-unknown-linux-gnu/release/veloren-voxygen-compressed
mv target/aarch64-unknown-linux-gnu/release/xindeler-server-cli-compressed \
   target/aarch64-unknown-linux-gnu/release/xindeler-server-cli
mv target/aarch64-unknown-linux-gnu/release/veloren-voxygen-compressed \
   target/aarch64-unknown-linux-gnu/release/veloren-voxygen
