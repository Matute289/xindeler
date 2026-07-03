#!/bin/bash
# LEGACY (BL-82): unreferenced by our workflows; still packages the voxygen binary,
# which this workspace no longer builds. Kept only as an upstream reference.
export VELOREN_USERDATA_STRATEGY=executable;
time cargo build --release --no-default-features --features default-publish;

objcopy --compress-debug-sections=zlib target/release/xindeler-server-cli target/release/xindeler-server-cli-compressed
objcopy --compress-debug-sections=zlib target/release/veloren-voxygen target/release/veloren-voxygen-compressed
mv target/release/xindeler-server-cli-compressed target/release/xindeler-server-cli
mv target/release/veloren-voxygen-compressed target/release/veloren-voxygen
