#!/usr/bin/env bash
# BL-82 (EM-1.3) — Veloren→Xindeler rebranding, executed as a SCRIPT so it can be
# re-run after every upstream sync (gitlab merges reintroduce `veloren-*` names;
# this restores ours mechanically). Idempotent: running it twice yields no diff.
# PROCEDURE: run this script, then `cargo fmt --all` (substitution can leave overlong
# lines that rustfmt re-wraps) — that pair reproduces the committed tree exactly.
#
# Source of truth for the mapping: docs/design/specs/2026-07-02-veloren-xindeler-mapper.md §B.
#
# SCOPE — what it renames:
#   * Cargo package names + dependent `package = "..."` keys + profile-override
#     sections + `--bin`/artifact references (CI, Docker, cargo aliases, docs)
#   * Rust identifier forms (`veloren_x` → `xindeler_x`) in the BUILT logic
#     crates' sources (a crate's tests/benches/bins import it by lib name)
#   * dylib hot-reload init strings (they derive from the package name)
# NEVER touches (upstream/ABI/runtime contracts):
#   * assets/** file names or load paths            * voxygen/** except anim + i18n-helpers
#   * veloren_plugin_* (WASM plugin ABI)            * DB schema / migrations
#   * wire-protocol magic/version strings           * internal CamelCase types (e.g. VelorenConnection)
#   * userdata/config dir names (separate item, needs a data-migration step — Mapper §A)
set -euo pipefail
cd "$(dirname "$0")/.."

python3 - <<'EOF'
import os, sys

# Package renames (hyphen form). Underscore forms are derived automatically.
# Keep in sync with Mapper §B. Replacement runs longest-key-first.
MAP = {
    "veloren-voxygen-i18n-helpers": "xindeler-i18n-helpers",
    "veloren-voxygen-anim":         "xindeler-anim",
    "veloren-network-protocol":     "xindeler-network-protocol",
    "veloren-common-assets":        "xindeler-common-assets",
    "veloren-common-base":          "xindeler-common-base",
    "veloren-common-dynlib":        "xindeler-common-dynlib",
    "veloren-common-ecs":           "xindeler-common-ecs",
    "veloren-common-frontend":      "xindeler-common-frontend",
    "veloren-common-i18n":          "xindeler-common-i18n",
    "veloren-common-net":           "xindeler-common-net",
    "veloren-common-state":         "xindeler-common-state",
    "veloren-common-systems":       "xindeler-common-systems",
    "veloren-query-server":         "xindeler-query-server",
    "veloren-client-i18n":          "xindeler-client-i18n",
    "veloren-server-agent":         "xindeler-server-agent",
    "veloren-server-cli":           "xindeler-server-cli",
    "veloren-botclient":            "xindeler-botclient",   # clap display name (client bot bin)
    "veloren-client":               "xindeler-client-core", # `xindeler-client` = the Bevy binary
    "veloren-server":               "xindeler-server-core", # `xindeler-server-app` = the Bevy shell
    "veloren-network":              "xindeler-network",
    "veloren-common":               "xindeler-common",
    "veloren-rtsim":                "xindeler-rtsim",
    "veloren-world":                "xindeler-world",
}
FULL = dict(MAP)
FULL.update({k.replace("-", "_"): v.replace("-", "_") for k, v in MAP.items()})
KEYS = sorted(FULL, key=len, reverse=True)   # longest first: -server-cli before -server

# (scope_path, recursive, extensions) — None ext = exact file
SCOPES = [
    ("Cargo.toml", False, None),
    (".cargo/config.toml", False, None),
    ("CLAUDE.md", False, None), ("AGENTS.md", False, None),
    (".github", True, (".yml", ".sh")),
    (".claude/skills", True, (".md",)), (".agents/skills", True, (".md",)),
    (".claude/agents", True, (".md",)), (".codex/agents", True, (".toml",)),
    ("bevy", True, (".toml", ".rs")),
    # built logic crates (voxygen/ itself is frozen; only its two logic subcrates)
    ("client", True, (".toml", ".rs")),
    ("common", True, (".toml", ".rs")),
    ("network", True, (".toml", ".rs")),
    ("rtsim", True, (".toml", ".rs")),
    ("server", True, (".toml", ".rs")),
    ("server-cli", True, (".toml", ".rs", ".yml")),
    ("server-cli/Dockerfile", False, None),
    ("server-cli/Dockerfile.vps", False, None),
    ("world", True, (".toml", ".rs")),
    ("voxygen/anim", True, (".toml", ".rs")),
    ("voxygen/i18n-helpers", True, (".toml", ".rs")),
]
SKIP_PARTS = {".git", "target", "worktrees", "node_modules"}
SELF = os.path.abspath("tools/xindeler-rename.sh")

def rewrite(path):
    if os.path.abspath(path) == SELF:
        return 0
    try:
        with open(path, encoding="utf-8") as f:
            s = f.read()
    except (UnicodeDecodeError, FileNotFoundError):
        return 0
    out = s
    for k in KEYS:
        out = out.replace(k, FULL[k])
    if out != s:
        with open(path, "w", encoding="utf-8") as f:
            f.write(out)
        return 1
    return 0

changed = 0
for scope, rec, exts in SCOPES:
    if not os.path.exists(scope):
        continue
    if not rec:
        changed += rewrite(scope)
        continue
    for dirpath, dirnames, filenames in os.walk(scope):
        dirnames[:] = [d for d in dirnames if d not in SKIP_PARTS]
        for fn in filenames:
            if exts and not fn.endswith(exts):
                continue
            changed += rewrite(os.path.join(dirpath, fn))

print(f"xindeler-rename: {changed} file(s) rewritten" if changed
      else "xindeler-rename: nothing to do (idempotent no-op)")
EOF

# Refresh the lockfile so the renamed packages resolve (never hand-merge the lock).
cargo metadata --format-version 1 >/dev/null
echo "xindeler-rename: done (Cargo.lock refreshed)"
