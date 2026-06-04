#!/bin/bash
export VELOREN_USERDATA_STRATEGY=executable;
time cargo build --release --no-default-features --features default-publish;

objcopy --compress-debug-sections=zlib target/release/veloren-server-cli target/release/veloren-server-cli-compressed
objcopy --compress-debug-sections=zlib target/release/veloren-voxygen target/release/veloren-voxygen-compressed
mv target/release/veloren-server-cli-compressed target/release/veloren-server-cli
mv target/release/veloren-voxygen-compressed target/release/veloren-voxygen
