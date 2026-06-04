# GitLab → GitHub CI/CD Migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Migrate all Veloren fork CI/CD from GitLab to GitHub Actions, with persistent data storage for translations/benchmarks and cleanup of all upstream references.

**Architecture:** Three independent phases — Phase A (foundation: scripts + basic workflows) delivers a working PR check pipeline; Phase B (build & publish) delivers release binaries and Docker on GHCR; Phase C (data persistence) delivers translation analysis and benchmark tracking with PostgreSQL service containers.

**Tech Stack:** GitHub Actions, Bash, PostgreSQL 15 (service container), Docker/GHCR, `actions-rust-lang/setup-rust-toolchain`, `Swatinem/rust-cache`, `peaceiris/actions-gh-pages`, `softprops/action-gh-release`

---

## Phase A — Foundation

### Task A1: Code cleanup (Discord URLs + docker-compose.yml)

**Files:**
- Modify: `world/src/site/mod.rs:236`
- Modify: `common/state/src/plugin/memory_manager.rs:112`
- Modify: `server-cli/docker-compose.yml:5`

- [ ] **Step 1: Remove Discord URL from world/src/site/mod.rs**

In `world/src/site/mod.rs`, find and remove line 236 (the comment with the discord URL). The surrounding context looks like:

```rust
            // Temporary solution for giving giant_tree's leaves enough space to be painted correctly
            // TODO: This will have to be replaced by a system as described on discord :
            // https://discord.com/channels/449602562165833758/450064928720814081/937044837461536808
            + if self
```

Remove only the `// https://discord.com/...` line. The file after edit:

```rust
            // Temporary solution for giving giant_tree's leaves enough space to be painted correctly
            // TODO: This will have to be replaced by a system as described on discord
            + if self
```

- [ ] **Step 2: Remove Discord URL from common/state/src/plugin/memory_manager.rs**

In `common/state/src/plugin/memory_manager.rs`, find and remove line 112. Context:

```rust
            // The called closure can't escape the reference because it must be callable for
            // any set of lifetimes. Variance of the lifetime parameters in EcsWorld are
            // not an issue for the same reason:
            // https://discord.com/channels/273534239310479360/592856094527848449/1111018259815342202
            unsafe { ptr.as_ref() }
```

Remove only the `// https://discord.com/...` line. After edit:

```rust
            // The called closure can't escape the reference because it must be callable for
            // any set of lifetimes. Variance of the lifetime parameters in EcsWorld are
            // not an issue for the same reason.
            unsafe { ptr.as_ref() }
```

- [ ] **Step 3: Update docker-compose.yml image URL**

In `server-cli/docker-compose.yml` line 5, change:
```yaml
    image: registry.gitlab.com/veloren/veloren/server-cli:weekly
```
to:
```yaml
    image: ghcr.io/mgrinberg/veloren/server-cli:weekly
```

- [ ] **Step 4: Commit**

```bash
git add world/src/site/mod.rs \
        common/state/src/plugin/memory_manager.rs \
        server-cli/docker-compose.yml
git commit -m "chore: remove Discord URLs from comments, update docker-compose to GHCR"
```

---

### Task A2: Create ci-data/ directory with CSV headers

**Files:**
- Create: `ci-data/translations.csv`
- Create: `ci-data/benchmarks.csv`

- [ ] **Step 1: Create ci-data/translations.csv with header**

Create `ci-data/translations.csv`:
```
country_code,file_name,translation_key,status,git_commit
```

- [ ] **Step 2: Create ci-data/benchmarks.csv with header**

Create `ci-data/benchmarks.csv`:
```
group,function,value,throughput_num,throughput_type,sample_measured_value,unit,iteration_count,git_commit,branch,recorded_at
```

- [ ] **Step 3: Add .gitkeep comment and verify**

```bash
wc -l ci-data/translations.csv ci-data/benchmarks.csv
# Expected: 1 ci-data/translations.csv  1 ci-data/benchmarks.csv
```

- [ ] **Step 4: Commit**

```bash
git add ci-data/
git commit -m "ci: add ci-data directory with empty CSV files for CI persistence"
```

---

### Task A3: Create GitHub issue template

**Files:**
- Create: `.github/ISSUE_TEMPLATE/bug_report.md`

- [ ] **Step 1: Create the directory**

```bash
mkdir -p .github/ISSUE_TEMPLATE
```

- [ ] **Step 2: Write the issue template**

Create `.github/ISSUE_TEMPLATE/bug_report.md` with this exact content:

```markdown
---
name: Bug Report
about: Report a bug or crash in Veloren
labels: "type::bug, status::needs investigation"
---

<!--
Before opening a new issue, please search for existing reports:
https://github.com/mgrinberg/veloren/issues?q=is%3Aissue+label%3Atype%3A%3Abug
https://github.com/mgrinberg/veloren/issues?q=is%3Aissue+label%3Atype%3A%3Acrash
-->

### Summary

(Summarize the bug encountered concisely)

### Steps to reproduce

(How one can reproduce the issue - this is very important)

### Relevant logs and/or screenshots

<details>
<summary>Logs and/or screenshots of the issue</summary>
<pre>

(Paste any relevant logs - please use code blocks (```) to format console output,
logs, and code as it's tough to read otherwise.)

</pre>
</details>

#### System details

(Include important system details like OS and in case it's a graphical issue the GPU)

#### Veloren version

(What version the bug happened e.g. Nightly, Stable 0.X.0, main)
```

- [ ] **Step 3: Commit**

```bash
git add .github/ISSUE_TEMPLATE/bug_report.md
git commit -m "ci: add GitHub issue template (adapted from GitLab)"
```

---

### Task A4: Create .github/scripts/ (all 12 scripts)

**Files:**
- Create: `.github/scripts/env.sh`
- Create: `.github/scripts/code-quality.sh`
- Create: `.github/scripts/security.sh`
- Create: `.github/scripts/unittest.sh`
- Create: `.github/scripts/translation.sh`
- Create: `.github/scripts/benchmark.sh`
- Create: `.github/scripts/coverage.sh`
- Create: `.github/scripts/linux-x86_64.sh`
- Create: `.github/scripts/linux-aarch64.sh`
- Create: `.github/scripts/windows-x86_64.sh`
- Create: `.github/scripts/util.sh`
- Create: `.github/scripts/plugin.sh`

- [ ] **Step 1: Create directory**

```bash
mkdir -p .github/scripts
```

- [ ] **Step 2: Create env.sh (adapted — SHADERC_LIB_DIR removed)**

```bash
#!/bin/bash
# Export default env variables in CI.
export DISABLE_GIT_LFS_CHECK=true;
export VELOREN_ASSETS="assets";

# When updating RUSTFLAGS here, windows-x86_64.sh must
# also be updated as it sets them independently.
export RUSTFLAGS="-D warnings";

export CARGO_INCREMENTAL=0;
```

- [ ] **Step 3: Create code-quality.sh (direct copy)**

```bash
#!/bin/bash
# cargo clippy is a superset of cargo check,
# so we don't check manually.

time cargo clippy \
    --all-targets \
    --locked \
    --features="bin_cmd_doc_gen,bin_compression,bin_csv,bin_graphviz,bin_bot,bin_asset_migrate,asset_tweak,bin,stat,cli" \
    -- -D warnings &&

# Ensure that the veloren-voxygen default-publish feature builds as it excludes some default features.
time cargo clippy -p \
    veloren-voxygen --locked \
    --no-default-features \
    --features="default-publish" \
    -- -D warnings &&

# Ensure that test-server compiles.
time cargo clippy --locked --bin veloren-server-cli --no-default-features -F simd  -- -D warnings &&
time cargo fmt --all -- --check;
```

- [ ] **Step 4: Create security.sh (direct copy)**

```bash
#!/bin/bash
time cargo audit;
```

- [ ] **Step 5: Create unittest.sh (direct copy)**

```bash
#!/bin/bash
VELOREN_ASSETS="$(pwd)/assets";
export VELOREN_ASSETS;

time cargo test \
    --package veloren-common-assets asset_tweak::tests \
    --features asset_tweak --lib &&
time cargo test;
```

- [ ] **Step 6: Create translation.sh (direct copy)**

```bash
#!/bin/bash
VELOREN_ASSETS="$(pwd)/assets";
export VELOREN_ASSETS;

time cargo run --bin i18n_csv --features="stat";
```

- [ ] **Step 7: Create benchmark.sh (direct copy)**

```bash
#!/bin/bash
time cargo bench;
```

- [ ] **Step 8: Create coverage.sh (direct copy)**

```bash
#!/bin/bash
echo "modifying files in 5s, ctrl+c to abort" && sleep 5;
find ./* -name "Cargo.toml" -exec sed -i -E 's/, *"simd"|"simd" *,|"simd"//g' {} \;
export VELOREN_ASSETS="$(pwd)/assets";
time cargo tarpaulin --skip-clean -v --engine llvm -- --test-threads=2;
```

- [ ] **Step 9: Create linux-x86_64.sh (direct copy)**

```bash
#!/bin/bash
export VELOREN_USERDATA_STRATEGY=executable;
time cargo build --release --no-default-features --features default-publish;

objcopy --compress-debug-sections=zlib target/release/veloren-server-cli target/release/veloren-server-cli-compressed
objcopy --compress-debug-sections=zlib target/release/veloren-voxygen target/release/veloren-voxygen-compressed
mv target/release/veloren-server-cli-compressed target/release/veloren-server-cli
mv target/release/veloren-voxygen-compressed target/release/veloren-voxygen
```

- [ ] **Step 10: Create linux-aarch64.sh (direct copy)**

```bash
#!/bin/bash
export VELOREN_USERDATA_STRATEGY=executable;
export PKG_CONFIG="/usr/bin/aarch64-linux-gnu-pkg-config";
time cargo build --target=aarch64-unknown-linux-gnu --release --no-default-features --features default-publish;

aarch64-linux-gnu-objcopy --compress-debug-sections=zlib \
    target/aarch64-unknown-linux-gnu/release/veloren-server-cli \
    target/aarch64-unknown-linux-gnu/release/veloren-server-cli-compressed
aarch64-linux-gnu-objcopy --compress-debug-sections=zlib \
    target/aarch64-unknown-linux-gnu/release/veloren-voxygen \
    target/aarch64-unknown-linux-gnu/release/veloren-voxygen-compressed
mv target/aarch64-unknown-linux-gnu/release/veloren-server-cli-compressed \
   target/aarch64-unknown-linux-gnu/release/veloren-server-cli
mv target/aarch64-unknown-linux-gnu/release/veloren-voxygen-compressed \
   target/aarch64-unknown-linux-gnu/release/veloren-voxygen
```

- [ ] **Step 11: Create windows-x86_64.sh (direct copy)**

```bash
#!/bin/bash
update-alternatives --set x86_64-w64-mingw32-gcc "/usr/bin/x86_64-w64-mingw32-gcc-posix";
update-alternatives --set x86_64-w64-mingw32-g++ "/usr/bin/x86_64-w64-mingw32-g++-posix";
export VELOREN_USERDATA_STRATEGY=executable;

# RUSTFLAGS is set here in addition to env.sh due to https://github.com/rust-lang/cargo/issues/5376
export RUSTFLAGS="-D warnings -C link-arg=-lpsapi";

time cargo build --target=x86_64-pc-windows-gnu --release --no-default-features --features "default-publish";
```

- [ ] **Step 12: Create util.sh (adapted for GitHub Actions env vars)**

```bash
#!/bin/sh

### Returns the Docker tag to publish.
### release-tag => <release-tag> (e.g. v1.2.3)
### schedule    => nightly
### main push   => master
### else        => ""
publishdockertag() {
  export PUBLISH_DOCKER_TAG="";

  # GitHub Actions uses GITHUB_REF_TYPE=tag and GITHUB_REF_NAME=v1.2.3
  TAG_REGEX='^v[0-9]+\.[0-9]+\.[0-9]+$'
  if [ "${GITHUB_REF_TYPE}" = "tag" ] && echo "${GITHUB_REF_NAME}" | grep -Eq "${TAG_REGEX}"; then
    export PUBLISH_DOCKER_TAG="${GITHUB_REF_NAME}";
    return 0
  fi

  # Schedule event
  if [ "${GITHUB_EVENT_NAME}" = "schedule" ]; then
    export PUBLISH_DOCKER_TAG="nightly";
    return 0;
  fi

  # Push to main branch
  if [ "${GITHUB_EVENT_NAME}" = "push" ] && [ "${GITHUB_REF_NAME}" = "main" ]; then
    export PUBLISH_DOCKER_TAG="master";
    return 0;
  fi
}
```

- [ ] **Step 13: Create plugin.sh (direct copy)**

```bash
#!/bin/bash
time cargo build --example=hello --target=wasm32-wasi;
```

- [ ] **Step 14: Make all scripts executable**

```bash
chmod +x .github/scripts/*.sh
```

- [ ] **Step 15: Commit**

```bash
git add .github/scripts/
git commit -m "ci: add .github/scripts/ migrated from .gitlab/scripts/"
```

---

### Task A5: Create DB SQL files

**Files:**
- Create: `.github/scripts/db/schema.sql`
- Create: `.github/scripts/db/import_translations.sql`
- Create: `.github/scripts/db/export_translations.sql`
- Create: `.github/scripts/db/import_benchmarks.sql`
- Create: `.github/scripts/db/export_benchmarks.sql`

- [ ] **Step 1: Create directory**

```bash
mkdir -p .github/scripts/db
```

- [ ] **Step 2: Create schema.sql**

```sql
-- CI data schema for Veloren fork
-- Mirrors the structure used by the upstream cidb.veloren.net

CREATE TABLE IF NOT EXISTS translations_stage (
  country_code TEXT,
  file_name    TEXT,
  translation_key TEXT,
  status       TEXT,
  git_commit   TEXT
);

CREATE TABLE IF NOT EXISTS translations (
  country_code    TEXT,
  file_name       TEXT,
  translation_key TEXT,
  status          TEXT,
  git_commit      TEXT,
  loaded_at       TIMESTAMP DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS benchmarks (
  "group"                TEXT,
  "function"             TEXT,
  value                  NUMERIC,
  throughput_num         NUMERIC,
  throughput_type        TEXT,
  sample_measured_value  NUMERIC,
  unit                   TEXT,
  iteration_count        INTEGER,
  git_commit             TEXT,
  branch                 TEXT,
  recorded_at            TIMESTAMP DEFAULT NOW()
);

CREATE OR REPLACE PROCEDURE public.load_translations_from_stage()
LANGUAGE plpgsql AS $$
BEGIN
  INSERT INTO translations (country_code, file_name, translation_key, status, git_commit)
  SELECT country_code, file_name, translation_key, status, git_commit
  FROM translations_stage;
  TRUNCATE translations_stage;
END;
$$;
```

- [ ] **Step 3: Create import_translations.sql**

```sql
-- Import historical translation data from ci-data/translations.csv into the DB.
-- Run from the repo root so relative path resolves correctly.
\copy translations (country_code, file_name, translation_key, status, git_commit)
  FROM 'ci-data/translations.csv' CSV HEADER;
```

- [ ] **Step 4: Create export_translations.sql**

```sql
-- Export all translation data from DB back to ci-data/translations.csv.
-- Run from the repo root so relative path resolves correctly.
\copy (
  SELECT country_code, file_name, translation_key, status, git_commit
  FROM translations
  ORDER BY country_code, file_name, translation_key
) TO 'ci-data/translations.csv' CSV HEADER;
```

- [ ] **Step 5: Create import_benchmarks.sql**

```sql
-- Import historical benchmark data from ci-data/benchmarks.csv into the DB.
\copy benchmarks ("group", "function", value, throughput_num, throughput_type,
                  sample_measured_value, unit, iteration_count, git_commit, branch, recorded_at)
  FROM 'ci-data/benchmarks.csv' CSV HEADER;
```

- [ ] **Step 6: Create export_benchmarks.sql**

```sql
-- Export all benchmark data from DB back to ci-data/benchmarks.csv.
\copy (
  SELECT "group", "function", value, throughput_num, throughput_type,
         sample_measured_value, unit, iteration_count, git_commit, branch, recorded_at
  FROM benchmarks
  ORDER BY recorded_at DESC, "group", "function"
) TO 'ci-data/benchmarks.csv' CSV HEADER;
```

- [ ] **Step 7: Commit**

```bash
git add .github/scripts/db/
git commit -m "ci: add DB SQL schema and import/export scripts for CI data persistence"
```

---

### Task A6: Create check.yml workflow

**Files:**
- Create: `.github/workflows/check.yml`

- [ ] **Step 1: Create the workflow**

```yaml
name: Check

on:
  pull_request:
    paths:
      - '**/*.rs'
      - '**/*.glsl'
      - '**/*.toml'
      - '**/*.ron'
      - '**/*.ftl'
      - 'Cargo.lock'
      - 'rust-toolchain'
      - '.github/workflows/check.yml'
      - '.github/scripts/code-quality.sh'
      - '.github/scripts/security.sh'
      - '.github/scripts/env.sh'
      - '.cargo/config.toml'
      - '.rustfmt.toml'
      - 'clippy.toml'

env:
  CARGO_TERM_COLOR: always

jobs:
  code-quality:
    name: Clippy + fmt
    runs-on: ubuntu-22.04
    steps:
      - uses: actions/checkout@v4
        with:
          lfs: true

      - uses: actions-rust-lang/setup-rust-toolchain@v1
        with:
          components: rustfmt,clippy

      - uses: Swatinem/rust-cache@v2

      - name: Install system dependencies
        run: |
          sudo apt-get update -qq
          sudo apt-get install -y --no-install-recommends \
            libshaderc-dev \
            pkg-config \
            libfontconfig1-dev \
            libudev-dev \
            libasound2-dev

      - name: Run clippy and fmt
        run: |
          source ./.github/scripts/env.sh
          source ./.github/scripts/code-quality.sh

  security:
    name: Security audit
    runs-on: ubuntu-22.04
    continue-on-error: true
    steps:
      - uses: actions/checkout@v4

      - uses: actions-rust-lang/setup-rust-toolchain@v1

      - uses: Swatinem/rust-cache@v2

      - name: Install cargo-audit
        run: cargo install cargo-audit --locked

      - name: Run security audit
        run: source ./.github/scripts/security.sh
```

- [ ] **Step 2: Verify YAML is valid**

```bash
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/check.yml'))" && echo "YAML valid"
```

Expected: `YAML valid`

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/check.yml
git commit -m "ci: add check.yml (clippy + fmt + security audit)"
```

---

### Task A7: Create test.yml workflow

**Files:**
- Create: `.github/workflows/test.yml`

- [ ] **Step 1: Create the workflow**

```yaml
name: Test

on:
  pull_request:
    paths:
      - '**/*.rs'
      - '**/*.ron'
      - '**/*.toml'
      - '**/*.ftl'
      - 'Cargo.lock'
      - 'rust-toolchain'
      - 'assets/**'
      - '.github/workflows/test.yml'
      - '.github/scripts/unittest.sh'

env:
  CARGO_TERM_COLOR: always

jobs:
  unit-tests:
    name: Unit tests
    runs-on: ubuntu-22.04
    steps:
      - uses: actions/checkout@v4
        with:
          lfs: true
          fetch-depth: 0

      - uses: actions-rust-lang/setup-rust-toolchain@v1

      - uses: Swatinem/rust-cache@v2

      - name: Install system dependencies
        run: |
          sudo apt-get update -qq
          sudo apt-get install -y --no-install-recommends \
            libshaderc-dev \
            pkg-config \
            libfontconfig1-dev \
            libudev-dev \
            libasound2-dev

      - name: Run unit tests
        run: source ./.github/scripts/unittest.sh
```

- [ ] **Step 2: Verify YAML**

```bash
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/test.yml'))" && echo "YAML valid"
```

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/test.yml
git commit -m "ci: add test.yml (cargo test)"
```

---

### Task A8: Create docs.yml workflow

**Files:**
- Create: `.github/workflows/docs.yml`

- [ ] **Step 1: Create the workflow**

```yaml
name: Docs

on:
  push:
    branches: [main]
  workflow_dispatch:

env:
  CARGO_TERM_COLOR: always

jobs:
  build-and-deploy:
    name: Build and publish rustdoc
    runs-on: ubuntu-22.04
    permissions:
      contents: write

    steps:
      - uses: actions/checkout@v4
        with:
          lfs: true

      - uses: actions-rust-lang/setup-rust-toolchain@v1

      - uses: Swatinem/rust-cache@v2

      - name: Install system dependencies
        run: |
          sudo apt-get update -qq
          sudo apt-get install -y --no-install-recommends \
            libshaderc-dev \
            pkg-config \
            libfontconfig1-dev \
            libudev-dev \
            libasound2-dev

      - name: Build rustdoc
        run: |
          RUSTDOCFLAGS="--enable-index-page -Zunstable-options" \
          cargo doc --no-deps --document-private-items

      - name: Deploy to GitHub Pages
        uses: peaceiris/actions-gh-pages@v4
        with:
          github_token: ${{ secrets.GITHUB_TOKEN }}
          publish_dir: ./target/doc
          publish_branch: gh-pages
          force_orphan: true
```

- [ ] **Step 2: Verify YAML**

```bash
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/docs.yml'))" && echo "YAML valid"
```

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/docs.yml
git commit -m "ci: add docs.yml (cargo doc → GitHub Pages)"
```

---

## Phase B — Build & Publish

### Task B1: Create build.yml workflow (multi-platform)

**Files:**
- Create: `.github/workflows/build.yml`

- [ ] **Step 1: Create the workflow**

```yaml
name: Build

on:
  push:
    branches: [main]
  workflow_dispatch:

env:
  CARGO_TERM_COLOR: always

jobs:
  build:
    name: Build ${{ matrix.target }}
    runs-on: ${{ matrix.os }}
    strategy:
      fail-fast: false
      matrix:
        include:
          - target: linux-x86_64
            os: ubuntu-22.04
            script: .github/scripts/linux-x86_64.sh
            artifact-path: |
              target/release/veloren-server-cli
              target/release/veloren-voxygen

          - target: linux-aarch64
            os: ubuntu-22.04
            script: .github/scripts/linux-aarch64.sh
            artifact-path: |
              target/aarch64-unknown-linux-gnu/release/veloren-server-cli
              target/aarch64-unknown-linux-gnu/release/veloren-voxygen

          - target: windows-x86_64
            os: ubuntu-22.04
            script: .github/scripts/windows-x86_64.sh
            artifact-path: |
              target/x86_64-pc-windows-gnu/release/veloren-server-cli.exe
              target/x86_64-pc-windows-gnu/release/veloren-voxygen.exe

          - target: macos-x86_64
            os: macos-13
            rust-target: x86_64-apple-darwin

          - target: macos-aarch64
            os: macos-latest
            rust-target: aarch64-apple-darwin

    steps:
      - uses: actions/checkout@v4
        with:
          lfs: true

      - uses: actions-rust-lang/setup-rust-toolchain@v1

      - uses: Swatinem/rust-cache@v2
        with:
          key: build-${{ matrix.target }}

      # --- Linux x86_64 ---
      - name: Install Linux x86_64 deps
        if: matrix.target == 'linux-x86_64'
        run: |
          sudo apt-get update -qq
          sudo apt-get install -y --no-install-recommends \
            libshaderc-dev pkg-config libfontconfig1-dev libudev-dev \
            libasound2-dev binutils

      # --- Linux aarch64 ---
      - name: Install Linux aarch64 cross-compile deps
        if: matrix.target == 'linux-aarch64'
        run: |
          sudo apt-get update -qq
          sudo apt-get install -y --no-install-recommends \
            gcc-aarch64-linux-gnu g++-aarch64-linux-gnu \
            libshaderc-dev pkg-config \
            binutils-aarch64-linux-gnu
          rustup target add aarch64-unknown-linux-gnu

      # --- Windows x86_64 ---
      - name: Install Windows cross-compile deps
        if: matrix.target == 'windows-x86_64'
        run: |
          sudo apt-get update -qq
          sudo apt-get install -y --no-install-recommends \
            gcc-mingw-w64-x86-64 g++-mingw-w64-x86-64 \
            libshaderc-dev pkg-config
          rustup target add x86_64-pc-windows-gnu

      # --- macOS ---
      - name: Install macOS deps (cmake + rust target)
        if: runner.os == 'macOS'
        run: |
          wget -q https://github.com/Kitware/CMake/releases/download/v3.31.8/cmake-3.31.8-macos-universal.tar.gz
          tar -xzf cmake-3.31.8-macos-universal.tar.gz
          echo "$(pwd)/cmake-3.31.8-macos-universal/CMake.app/Contents/bin" >> $GITHUB_PATH
          rustup target add ${{ matrix.rust-target }}

      # --- Build (Linux / Windows via script) ---
      - name: Build (Linux/Windows)
        if: runner.os == 'Linux'
        run: |
          source ./.github/scripts/env.sh
          source ./${{ matrix.script }}

      # --- Build (macOS native) ---
      - name: Build (macOS)
        if: runner.os == 'macOS'
        env:
          MACOSX_DEPLOYMENT_TARGET: "10.15"
          VELOREN_USERDATA_STRATEGY: executable
          VELOREN_ASSETS: "${{ github.workspace }}/assets"
          RUSTFLAGS: "-D warnings"
          CARGO_INCREMENTAL: "0"
        run: |
          cargo build --profile release \
            --no-default-features --features default-publish \
            --target ${{ matrix.rust-target }}
          cp target/${{ matrix.rust-target }}/release/veloren-server-cli .
          cp target/${{ matrix.rust-target }}/release/veloren-voxygen .

      # --- Upload artifacts (Linux/Windows) ---
      - name: Upload artifacts (Linux/Windows)
        if: runner.os == 'Linux'
        uses: actions/upload-artifact@v4
        with:
          name: veloren-${{ matrix.target }}
          path: |
            ${{ matrix.artifact-path }}
            assets/
            LICENSE
          retention-days: 7

      # --- Upload artifacts (macOS) ---
      - name: Upload artifacts (macOS)
        if: runner.os == 'macOS'
        uses: actions/upload-artifact@v4
        with:
          name: veloren-${{ matrix.target }}
          path: |
            veloren-server-cli
            veloren-voxygen
            assets/
            LICENSE
          retention-days: 7
```

- [ ] **Step 2: Verify YAML**

```bash
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/build.yml'))" && echo "YAML valid"
```

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/build.yml
git commit -m "ci: add build.yml (multi-platform: linux x86/aarch64, windows, macos)"
```

---

### Task B2: Create publish-docker.yml workflow

**Files:**
- Create: `.github/workflows/publish-docker.yml`

- [ ] **Step 1: Create the workflow**

```yaml
name: Publish Docker

on:
  push:
    branches: [main]
    tags: ['v*.*.*']
  workflow_dispatch:

jobs:
  publish-docker:
    name: Build and push server-cli to GHCR
    runs-on: ubuntu-22.04
    permissions:
      contents: read
      packages: write

    steps:
      - uses: actions/checkout@v4
        with:
          lfs: true

      - name: Determine Docker tag
        id: tag
        run: |
          source ./.github/scripts/util.sh
          publishdockertag
          echo "docker_tag=${PUBLISH_DOCKER_TAG}" >> "$GITHUB_OUTPUT"

      - name: Login to GitHub Container Registry
        if: steps.tag.outputs.docker_tag != ''
        uses: docker/login-action@v3
        with:
          registry: ghcr.io
          username: ${{ github.actor }}
          password: ${{ secrets.GITHUB_TOKEN }}

      - name: Set up Docker Buildx
        if: steps.tag.outputs.docker_tag != ''
        uses: docker/setup-buildx-action@v3

      - name: Build and push
        if: steps.tag.outputs.docker_tag != ''
        uses: docker/build-push-action@v5
        with:
          context: .
          file: server-cli/Dockerfile
          push: true
          tags: |
            ghcr.io/${{ github.repository_owner }}/veloren/server-cli:${{ steps.tag.outputs.docker_tag }}
          cache-from: type=gha
          cache-to: type=gha,mode=max
```

- [ ] **Step 2: Verify YAML**

```bash
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/publish-docker.yml'))" && echo "YAML valid"
```

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/publish-docker.yml
git commit -m "ci: add publish-docker.yml (server-cli image → GHCR)"
```

---

### Task B3: Create publish-release.yml workflow

**Files:**
- Create: `.github/workflows/publish-release.yml`

Note: This workflow runs all builds internally on tag push so artifacts are available within the same workflow run.

- [ ] **Step 1: Create the workflow**

```yaml
name: Publish Release

on:
  push:
    tags: ['v*.*.*']

env:
  CARGO_TERM_COLOR: always

jobs:
  build-linux-x86_64:
    name: Build linux-x86_64
    runs-on: ubuntu-22.04
    steps:
      - uses: actions/checkout@v4
        with: { lfs: true }
      - uses: actions-rust-lang/setup-rust-toolchain@v1
      - uses: Swatinem/rust-cache@v2
        with: { key: release-linux-x86_64 }
      - run: |
          sudo apt-get update -qq
          sudo apt-get install -y --no-install-recommends \
            libshaderc-dev pkg-config libfontconfig1-dev libudev-dev libasound2-dev binutils
      - run: |
          source .github/scripts/env.sh
          source .github/scripts/linux-x86_64.sh
      - uses: actions/upload-artifact@v4
        with:
          name: veloren-linux-x86_64
          path: |
            target/release/veloren-server-cli
            target/release/veloren-voxygen
            assets/
            LICENSE
          retention-days: 1

  build-linux-aarch64:
    name: Build linux-aarch64
    runs-on: ubuntu-22.04
    steps:
      - uses: actions/checkout@v4
        with: { lfs: true }
      - uses: actions-rust-lang/setup-rust-toolchain@v1
      - uses: Swatinem/rust-cache@v2
        with: { key: release-linux-aarch64 }
      - run: |
          sudo apt-get update -qq
          sudo apt-get install -y --no-install-recommends \
            gcc-aarch64-linux-gnu g++-aarch64-linux-gnu \
            libshaderc-dev pkg-config binutils-aarch64-linux-gnu
          rustup target add aarch64-unknown-linux-gnu
      - run: |
          source .github/scripts/env.sh
          source .github/scripts/linux-aarch64.sh
      - uses: actions/upload-artifact@v4
        with:
          name: veloren-linux-aarch64
          path: |
            target/aarch64-unknown-linux-gnu/release/veloren-server-cli
            target/aarch64-unknown-linux-gnu/release/veloren-voxygen
            assets/
            LICENSE
          retention-days: 1

  build-windows-x86_64:
    name: Build windows-x86_64
    runs-on: ubuntu-22.04
    steps:
      - uses: actions/checkout@v4
        with: { lfs: true }
      - uses: actions-rust-lang/setup-rust-toolchain@v1
      - uses: Swatinem/rust-cache@v2
        with: { key: release-windows-x86_64 }
      - run: |
          sudo apt-get update -qq
          sudo apt-get install -y --no-install-recommends \
            gcc-mingw-w64-x86-64 g++-mingw-w64-x86-64 libshaderc-dev pkg-config
          rustup target add x86_64-pc-windows-gnu
      - run: |
          source .github/scripts/env.sh
          source .github/scripts/windows-x86_64.sh
      - uses: actions/upload-artifact@v4
        with:
          name: veloren-windows-x86_64
          path: |
            target/x86_64-pc-windows-gnu/release/veloren-server-cli.exe
            target/x86_64-pc-windows-gnu/release/veloren-voxygen.exe
            assets/
            LICENSE
          retention-days: 1

  build-macos-x86_64:
    name: Build macos-x86_64
    runs-on: macos-13
    steps:
      - uses: actions/checkout@v4
        with: { lfs: true }
      - uses: actions-rust-lang/setup-rust-toolchain@v1
      - uses: Swatinem/rust-cache@v2
        with: { key: release-macos-x86_64 }
      - run: |
          wget -q https://github.com/Kitware/CMake/releases/download/v3.31.8/cmake-3.31.8-macos-universal.tar.gz
          tar -xzf cmake-3.31.8-macos-universal.tar.gz
          echo "$(pwd)/cmake-3.31.8-macos-universal/CMake.app/Contents/bin" >> $GITHUB_PATH
          rustup target add x86_64-apple-darwin
      - env:
          MACOSX_DEPLOYMENT_TARGET: "10.15"
          VELOREN_USERDATA_STRATEGY: executable
          VELOREN_ASSETS: "${{ github.workspace }}/assets"
          RUSTFLAGS: "-D warnings"
          CARGO_INCREMENTAL: "0"
        run: |
          cargo build --profile release \
            --no-default-features --features default-publish \
            --target x86_64-apple-darwin
          cp target/x86_64-apple-darwin/release/veloren-server-cli .
          cp target/x86_64-apple-darwin/release/veloren-voxygen .
      - uses: actions/upload-artifact@v4
        with:
          name: veloren-macos-x86_64
          path: |
            veloren-server-cli
            veloren-voxygen
            assets/
            LICENSE
          retention-days: 1

  build-macos-aarch64:
    name: Build macos-aarch64
    runs-on: macos-latest
    steps:
      - uses: actions/checkout@v4
        with: { lfs: true }
      - uses: actions-rust-lang/setup-rust-toolchain@v1
      - uses: Swatinem/rust-cache@v2
        with: { key: release-macos-aarch64 }
      - run: |
          wget -q https://github.com/Kitware/CMake/releases/download/v3.31.8/cmake-3.31.8-macos-universal.tar.gz
          tar -xzf cmake-3.31.8-macos-universal.tar.gz
          echo "$(pwd)/cmake-3.31.8-macos-universal/CMake.app/Contents/bin" >> $GITHUB_PATH
          rustup target add aarch64-apple-darwin
      - env:
          MACOSX_DEPLOYMENT_TARGET: "10.15"
          VELOREN_USERDATA_STRATEGY: executable
          VELOREN_ASSETS: "${{ github.workspace }}/assets"
          RUSTFLAGS: "-D warnings"
          CARGO_INCREMENTAL: "0"
        run: |
          cargo build --profile release \
            --no-default-features --features default-publish \
            --target aarch64-apple-darwin
          cp target/aarch64-apple-darwin/release/veloren-server-cli .
          cp target/aarch64-apple-darwin/release/veloren-voxygen .
      - uses: actions/upload-artifact@v4
        with:
          name: veloren-macos-aarch64
          path: |
            veloren-server-cli
            veloren-voxygen
            assets/
            LICENSE
          retention-days: 1

  create-release:
    name: Create GitHub Release
    runs-on: ubuntu-22.04
    needs:
      - build-linux-x86_64
      - build-linux-aarch64
      - build-windows-x86_64
      - build-macos-x86_64
      - build-macos-aarch64
    permissions:
      contents: write

    steps:
      - uses: actions/checkout@v4

      - name: Download all artifacts
        uses: actions/download-artifact@v4
        with:
          path: dist/

      - name: Package each platform into a zip
        run: |
          cd dist
          for dir in veloren-*/; do
            name="${dir%/}"
            zip -r "../${name}-${{ github.ref_name }}.zip" "${dir}"
            echo "Created ${name}-${{ github.ref_name }}.zip"
          done
          ls -lh ../*.zip

      - name: Create GitHub Release
        uses: softprops/action-gh-release@v2
        with:
          name: "Veloren ${{ github.ref_name }}"
          generate_release_notes: true
          files: '*.zip'
```

- [ ] **Step 2: Verify YAML**

```bash
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/publish-release.yml'))" && echo "YAML valid"
```

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/publish-release.yml
git commit -m "ci: add publish-release.yml (GitHub Release with all platform binaries on tag)"
```

---

## Phase C — Data Persistence

### Task C1: Create translation.yml workflow

**Files:**
- Create: `.github/workflows/translation.yml`

- [ ] **Step 1: Create the workflow**

```yaml
name: Translation Analysis

on:
  push:
    branches: [main]
    paths:
      - 'assets/voxygen/i18n/**'
      - '.github/workflows/translation.yml'
      - '.github/scripts/translation.sh'
  schedule:
    - cron: '0 0 * * 0'   # Every Sunday at midnight UTC
  workflow_dispatch:

env:
  CARGO_TERM_COLOR: always
  PGHOST: localhost
  PGPORT: 5432
  PGDATABASE: veloren_ci
  PGUSER: postgres
  PGPASSWORD: postgres

jobs:
  analyze-translations:
    name: Analyze i18n and persist data
    runs-on: ubuntu-22.04
    permissions:
      contents: write

    services:
      postgres:
        image: postgres:15
        env:
          POSTGRES_DB: veloren_ci
          POSTGRES_USER: postgres
          POSTGRES_PASSWORD: postgres
        ports:
          - 5432:5432
        options: >-
          --health-cmd pg_isready
          --health-interval 10s
          --health-timeout 5s
          --health-retries 5

    steps:
      - uses: actions/checkout@v4
        with:
          lfs: true
          token: ${{ secrets.GITHUB_TOKEN }}

      - uses: actions-rust-lang/setup-rust-toolchain@v1

      - uses: Swatinem/rust-cache@v2

      - name: Initialize DB schema
        run: psql -f .github/scripts/db/schema.sql

      - name: Import existing translation history into DB
        run: |
          if [ -s ci-data/translations.csv ]; then
            psql -f .github/scripts/db/import_translations.sql
          else
            echo "No existing translation history — starting fresh"
          fi

      - name: Run translation analysis
        run: |
          source .github/scripts/env.sh
          source .github/scripts/translation.sh

      - name: Load new translations into DB
        run: |
          if [ -f translation_analysis.csv ]; then
            psql -c "\copy translations_stage (country_code, file_name, translation_key, status, git_commit) \
                     FROM 'translation_analysis.csv' CSV HEADER"
            psql -c "CALL public.load_translations_from_stage();"
          else
            echo "translation_analysis.csv not generated — skipping DB load"
          fi

      - name: Export updated translations to ci-data/
        run: psql -f .github/scripts/db/export_translations.sql

      - name: Commit updated translation data
        run: |
          git config user.name "github-actions[bot]"
          git config user.email "github-actions[bot]@users.noreply.github.com"
          git add ci-data/translations.csv
          if git diff --staged --quiet; then
            echo "No translation data changes"
          else
            git commit -m "ci: update translation analysis data [skip ci]"
            git push
          fi
```

- [ ] **Step 2: Verify YAML**

```bash
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/translation.yml'))" && echo "YAML valid"
```

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/translation.yml
git commit -m "ci: add translation.yml (i18n analysis with PostgreSQL persistence)"
```

---

### Task C2: Create benchmarks.yml workflow

**Files:**
- Create: `.github/workflows/benchmarks.yml`

- [ ] **Step 1: Create the workflow**

```yaml
name: Benchmarks

on:
  push:
    branches: [main]
    paths:
      - '**/*.rs'
      - 'Cargo.lock'
      - '.github/workflows/benchmarks.yml'
      - '.github/scripts/benchmark.sh'
  schedule:
    - cron: '0 2 * * 0'   # Every Sunday at 02:00 UTC (after translation)
  workflow_dispatch:

env:
  CARGO_TERM_COLOR: always
  PGHOST: localhost
  PGPORT: 5432
  PGDATABASE: veloren_ci
  PGUSER: postgres
  PGPASSWORD: postgres

jobs:
  run-benchmarks:
    name: Run benchmarks and persist results
    runs-on: ubuntu-22.04
    permissions:
      contents: write

    services:
      postgres:
        image: postgres:15
        env:
          POSTGRES_DB: veloren_ci
          POSTGRES_USER: postgres
          POSTGRES_PASSWORD: postgres
        ports:
          - 5432:5432
        options: >-
          --health-cmd pg_isready
          --health-interval 10s
          --health-timeout 5s
          --health-retries 5

    steps:
      - uses: actions/checkout@v4
        with:
          lfs: true
          token: ${{ secrets.GITHUB_TOKEN }}

      - uses: actions-rust-lang/setup-rust-toolchain@v1

      - uses: Swatinem/rust-cache@v2

      - name: Install system dependencies
        run: |
          sudo apt-get update -qq
          sudo apt-get install -y --no-install-recommends \
            libshaderc-dev pkg-config libfontconfig1-dev libudev-dev libasound2-dev

      - name: Initialize DB schema
        run: psql -f .github/scripts/db/schema.sql

      - name: Import existing benchmark history into DB
        run: |
          if [ -s ci-data/benchmarks.csv ]; then
            psql -f .github/scripts/db/import_benchmarks.sql
          else
            echo "No existing benchmark history — starting fresh"
          fi

      - name: Run benchmarks
        run: source .github/scripts/benchmark.sh

      - name: Load new benchmark results into DB
        run: |
          COMMIT="${{ github.sha }}"
          BRANCH="${{ github.ref_name }}"
          # Combine all criterion new/*.csv files, adding git_commit and branch columns
          COMBINED=$(mktemp /tmp/benchmarks-XXXXXX.csv)
          echo "group,function,value,throughput_num,throughput_type,sample_measured_value,unit,iteration_count,git_commit,branch" > "${COMBINED}"
          find target/criterion -wholename "*/new/*.csv" | while read f; do
            tail -n +2 "${f}" | awk -F, -v c="${COMMIT}" -v b="${BRANCH}" \
              'NF > 0 { print $0 "," c "," b }' >> "${COMBINED}"
          done
          if [ "$(wc -l < "${COMBINED}")" -gt 1 ]; then
            psql -c "\copy benchmarks (\"group\", \"function\", value, throughput_num, \
                     throughput_type, sample_measured_value, unit, iteration_count, \
                     git_commit, branch) FROM '${COMBINED}' CSV HEADER"
          else
            echo "No benchmark results found in target/criterion"
          fi
          rm -f "${COMBINED}"

      - name: Export all benchmark data to ci-data/
        run: psql -f .github/scripts/db/export_benchmarks.sql

      - name: Commit updated benchmark data
        run: |
          git config user.name "github-actions[bot]"
          git config user.email "github-actions[bot]@users.noreply.github.com"
          git add ci-data/benchmarks.csv
          if git diff --staged --quiet; then
            echo "No benchmark data changes"
          else
            git commit -m "ci: update benchmark results [skip ci]"
            git push
          fi
```

- [ ] **Step 2: Verify YAML**

```bash
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/benchmarks.yml'))" && echo "YAML valid"
```

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/benchmarks.yml
git commit -m "ci: add benchmarks.yml (cargo bench with PostgreSQL persistence)"
```

---

## Final verification

- [ ] **Verify all workflow YAMLs are valid**

```bash
for f in .github/workflows/*.yml; do
  python3 -c "import yaml; yaml.safe_load(open('${f}'))" && echo "OK: ${f}" || echo "ERROR: ${f}"
done
```

Expected: all files print `OK: ...`

- [ ] **Verify all scripts are executable**

```bash
ls -la .github/scripts/*.sh | awk '{print $1, $9}' | grep -v "^-rwx"
```

Expected: empty output (all scripts have `x` bit set)

- [ ] **Verify ci-data CSV headers are correct**

```bash
head -1 ci-data/translations.csv
head -1 ci-data/benchmarks.csv
```

Expected:
```
country_code,file_name,translation_key,status,git_commit
group,function,value,throughput_num,throughput_type,sample_measured_value,unit,iteration_count,git_commit,branch,recorded_at
```

- [ ] **Verify no GitLab references remain (outside .git/ and docs/)**

```bash
grep -rn "gitlab\.com\|registry\.gitlab\|GITLAB_TOKEN\|CI_COMMIT\|CI_PIPELINE\|CI_DEFAULT_BRANCH\|CI_PROJECT\|CI_REGISTRY\|veloren-bot@veloren\|cidb\.veloren\|hgseehzjtsrghtjdcqw\|discord\.com" \
  --include="*.yml" --include="*.yaml" --include="*.sh" --include="*.rs" --include="*.md" \
  --exclude-dir=".git" --exclude-dir="docs" \
  /Users/mgrinberg/Workspace/RustroverProjects/veloren 2>/dev/null \
  | grep -v "Binary"
```

Expected: no output (or only the mirror.yml reference to `gitlab.com/veloren/veloren.git` which is intentional)

- [ ] **Final commit**

```bash
git add -A
git status   # confirm no unexpected files
git log --oneline -10
```

---

## Notes for future migration to own server

When a personal server and domain are ready, update these GitHub Secrets in repo Settings → Secrets:
- `DB_HOST` → your PostgreSQL host
- `DB_PORT` → your PostgreSQL port
- `DB_NAME` → your database name
- `DB_USER` → your DB user
- `DB_PASSWORD` → your DB password
- `DB_SSLMODE` → `verify-ca`
- `DB_SSLROOTCERT` → base64-encoded CA certificate

Also update `translation.yml` and `benchmarks.yml` `env:` block to read from these secrets instead of hardcoded `postgres`.
