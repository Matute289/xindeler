#!/usr/bin/env bash
# BL-82 (EM-1.2) — engine-isolation law, CI-enforced.
#
# The Veloren logic crates (upstream-merged) must NEVER depend on the Bevy
# engine layer, in either direction:
#   (a) no logic crate may depend (directly or transitively through other logic
#       crates, in any feature configuration, incl. dev/build deps) on bevy*,
#       wgpu*, winit, or any crate living under bevy/,
#   (b) no crate under bevy/ may be a dependency of a logic crate (same check,
#       stated from the other side),
#   (c) no logic-crate source file may `use`/reference the bevy namespace.
# Spec: docs/design/specs/2026-07-02-bevy-migration-design.md §3.1
set -euo pipefail
cd "$(dirname "$0")/.."

# (c) cheap source-level grep first: a `use bevy...` in a logic crate is illegal
# even if the dep graph is (momentarily) clean.
LOGIC_SRC=(client/src common/src common/*/src network/src network/protocol/src \
           rtsim/src server/src server/agent/src server-cli/src tools/*/src \
           world/src voxygen/anim/src voxygen/i18n-helpers/src)
if grep -rn --include='*.rs' -E '\bbevy(_[a-z_]+)?::' "${LOGIC_SRC[@]}" 2>/dev/null; then
    echo "ENGINE-ISOLATION VIOLATION (BL-82 §3.1): logic-crate source references the bevy namespace (see matches above)."
    exit 1
fi

# (d) [Q3]=B client purity: the Bevy client links logic crates as TYPE LIBRARIES
# only — any specs usage in it means sim state is leaking into the client
# (the only legal specs consumer in bevy/ is the server-side sim-bridge).
if grep -rn --include='*.rs' -E '\bspecs::|^use specs' bevy/xindeler-client/src 2>/dev/null; then
    echo "ENGINE-ISOLATION VIOLATION (BL-82 §2.1): the Bevy client must not use specs (type-library-only rule)."
    exit 1
fi

python3 - <<'EOF'
import json, subprocess, sys, os

# --all-features: the resolve graph must cover every feature configuration —
# CI builds with non-default features (bin_bot, bin_compression, ...), so a
# forbidden dep hidden behind one of those must still fail the gate.
meta = json.loads(subprocess.check_output(
    ["cargo", "metadata", "--format-version", "1", "--locked", "--all-features"]))

ws = {p["id"]: p for p in meta["packages"] if p["id"] in meta["workspace_members"]}
root = meta["workspace_root"]

def is_shell(pkg):          # lives under bevy/ → engine shell crate
    return pkg["manifest_path"].startswith(os.path.join(root, "bevy") + os.sep)

logic = {pid: p for pid, p in ws.items() if not is_shell(p)}
FORBIDDEN_PREFIXES = ("bevy", "wgpu", "winit")

# resolve node lookup: id -> direct dep ids. Include normal, build AND dev
# kinds — a bevy dev-dependency of a logic crate is just as illegal (dev edges
# only exist for workspace members, so this stays cheap and safe).
nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
def deps(pid):
    out = []
    for d in nodes[pid]["deps"]:
        kinds = {k.get("kind") for k in d["dep_kinds"]} or {None}
        if kinds & {None, "build", "dev"}:
            out.append(d["pkg"])
    return out

by_id = {p["id"]: p for p in meta["packages"]}
bad = []
for pid, pkg in sorted(logic.items(), key=lambda kv: kv[1]["name"]):
    seen, stack = set(), [pid]
    parent = {}                       # child -> parent, to reconstruct the chain
    while stack:
        cur = stack.pop()
        for dep in deps(cur):
            if dep in seen:
                continue
            seen.add(dep)
            parent[dep] = cur
            dp = by_id[dep]
            # NOTE: no workspace-membership guard here — a bevy/-tree crate
            # consumed as a non-member path dep must still be detected.
            if dp["name"].startswith(FORBIDDEN_PREFIXES) or is_shell(dp):
                chain, node = [dp["name"]], dep
                while node in parent:
                    node = parent[node]
                    chain.append(by_id[node]["name"])
                bad.append((pkg["name"], " -> ".join(reversed(chain))))
            else:
                stack.append(dep)

if bad:
    print("ENGINE-ISOLATION VIOLATION (BL-82 §3.1): logic crates must not depend on the engine layer:")
    for _, chain in sorted(set(bad)):
        print(f"  {chain}")
    sys.exit(1)

print(f"engine-isolation OK: {len(logic)} logic crates clean of "
      f"{'/'.join(FORBIDDEN_PREFIXES)} and bevy/* shell crates (all features, incl. dev/build deps)")
EOF
