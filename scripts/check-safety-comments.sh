#!/usr/bin/env bash
# Fails if any real unsafe site lacks a `// SAFETY:` comment within the 8
# lines above it. "Real" = unsafe block/impl/fn, excluding the mechanical
# `unsafe(export_name)`/`unsafe(no_mangle)` attributes Rust 2024 requires on
# hot-reload dylib exports (voxygen/anim, world plots, ...).
# Policy: docs/design/specs/2026-06-10-engine-improvements-design.md §B1
set -uo pipefail
cd "$(dirname "$0")/.."

fail=0
while IFS=: read -r file line _; do
    start=$(( line > 8 ? line - 8 : 1 ))
    if ! sed -n "${start},${line}p" "$file" | grep -q 'SAFETY'; then
        echo "MISSING SAFETY comment: ${file}:${line}"
        fail=1
    fi
done < <(grep -rnE 'unsafe \{|unsafe impl|unsafe fn' \
        --include='*.rs' --exclude-dir=target \
        client common network plugin rtsim server server-cli voxygen world \
        2>/dev/null \
    | grep -v 'unsafe(')

if [ "$fail" -ne 0 ]; then
    echo "FAIL: every real unsafe site needs a '// SAFETY:' comment (B1 policy)."
    exit 1
fi
echo "OK: all real unsafe sites carry SAFETY comments."
