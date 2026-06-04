# GitLab → GitHub CI/CD Migration Design

**Date:** 2026-06-03
**Status:** Approved
**Scope:** Full migration of Veloren fork CI/CD from GitLab to GitHub Actions, with persistent CI data storage and removal of all third-party references.

---

## Contexto

Matias tiene un fork personal de Veloren (juego RPG voxel en Rust). El proyecto upstream usa GitLab CI con infraestructura propia (`cidb.veloren.net`, Docker images privadas, `veloren-bot`). El fork migra toda la CI a GitHub Actions, usa GHCR para Docker, y mantiene solo un espejo desde el upstream de GitLab para mantenerse actualizado.

**Lo único de GitLab que permanece:** `mirror.yml` — sincronización horaria desde `gitlab.com/veloren/veloren` master hacia GitHub, para mantenerse actualizado con el upstream.

---

## Lo que se elimina

| Elemento | Ubicación | Acción |
|----------|-----------|--------|
| `gitlab-veloren-bot@veloren.net` | `publish.gitlab-ci.yml` | Reemplazado por `github-actions[bot]@users.noreply.github.com` |
| `registry.gitlab.com/veloren/*` | Todos los CI YMLs | Reemplazado por `ubuntu-22.04` + rust-toolchain action |
| `ci-db.crt` | `.gitlab/ci-db.crt` | No se incluye en GitHub CI (service container local, sin SSL) |
| Links GitLab en Bug template | `.gitlab/issue_templates/Bug.md` | Reemplazado por links a github.com/mgrinberg/veloren/issues |
| `registry.gitlab.com/veloren/veloren/server-cli` | `server-cli/docker-compose.yml` | → `ghcr.io/mgrinberg/veloren/server-cli` |
| Discord URL en comentario | `world/src/site/mod.rs:236` | Línea eliminada |
| Discord URL en comentario | `common/state/src/plugin/memory_manager.rs:112` | Línea eliminada |
| `CIDBPASSWORD`, `cidb.veloren.net`, `hgseehzjtsrghtjdcqw` | `build.gitlab-ci.yml` | No migrados (reemplazado por service container) |
| `GITLAB_TOKEN_WRITE` | `publish.gitlab-ci.yml` | No migrado |

---

## Estructura de Archivos Final

```
.github/
├── workflows/
│   ├── check.yml               # PR: clippy + fmt + cargo audit
│   ├── test.yml                # PR: cargo test
│   ├── build.yml               # main/tag: builds multi-plataforma
│   ├── publish-docker.yml      # main/tag: imagen servidor → GHCR
│   ├── publish-release.yml     # tag v*.*.*: GitHub Release con binarios
│   ├── translation.yml         # main: i18n análisis + persistencia CSV
│   ├── benchmarks.yml          # main/schedule: bench + persistencia CSV
│   ├── docs.yml                # main: cargo doc → GitHub Pages
│   ├── mirror.yml              # KEEP: espejo desde GitLab upstream
│   ├── no-pr.yml               # KEEP as-is
│   ├── check-source-branch.yml # KEEP as-is
│   └── sync-hotfix.yml         # KEEP as-is
├── scripts/
│   ├── env.sh                  # Adaptado (sin SHADERC_LIB_DIR hardcodeado)
│   ├── code-quality.sh         # Copiado de .gitlab/scripts/
│   ├── security.sh             # Copiado de .gitlab/scripts/
│   ├── unittest.sh             # Copiado de .gitlab/scripts/
│   ├── translation.sh          # Copiado de .gitlab/scripts/
│   ├── benchmark.sh            # Copiado de .gitlab/scripts/
│   ├── coverage.sh             # Copiado de .gitlab/scripts/
│   ├── linux-x86_64.sh         # Copiado de .gitlab/scripts/
│   ├── linux-aarch64.sh        # Copiado de .gitlab/scripts/
│   ├── windows-x86_64.sh       # Copiado de .gitlab/scripts/
│   ├── util.sh                 # Adaptado: publishdockertag → GHCR
│   ├── plugin.sh               # Copiado de .gitlab/scripts/
│   └── db/
│       ├── schema.sql          # Tablas: translations_stage, benchmarks
│       ├── import_translations.sql
│       ├── export_translations.sql
│       ├── import_benchmarks.sql
│       └── export_benchmarks.sql
└── ISSUE_TEMPLATE/
    └── bug_report.md           # Adaptado de .gitlab/issue_templates/Bug.md

ci-data/                        # Root del repo, commiteado a git
├── translations.csv            # Datos de traducciones persistidos entre CI runs
└── benchmarks.csv              # Datos de benchmarks persistidos entre CI runs
```

---

## 1. Workflows de CI

### Mapeo GitLab CI → GitHub Actions

| GitLab job | GitHub workflow | Trigger |
|------------|----------------|---------|
| `code-quality` | `check.yml` | `pull_request` con paths |
| `security` | `check.yml` | `pull_request` con paths |
| `unittests` | `test.yml` | `pull_request` con paths |
| `translation` (generación CSV) | `translation.yml` | push `main`, schedule semanal |
| `benchmarks` | `benchmarks.yml` | push `main`, schedule semanal |
| `coverage` | `test.yml` (job opcional) | push `main` |
| `linux-x86_64`, `linux-aarch64`, `windows-x86_64`, `macos-*` | `build.yml` | push `main`, tag `v*.*.*` |
| `docker` | `publish-docker.yml` | push `main`, tag `v*.*.*` |
| `gitlab_release` | `publish-release.yml` | tag `v*.*.*` solamente |
| `pages` | `docs.yml` | push `main` |
| `gittag` | No migrado (GitHub maneja tags nativamente) | — |

### check.yml — detalle

```yaml
on:
  pull_request:
    paths:
      - '**/*.rs'
      - '**/*.glsl'
      - '**/*.toml'
      - '**/*.ron'
      - 'Cargo.lock'
      - 'rust-toolchain'
      - '.github/workflows/check.yml'
      - '.github/scripts/*.sh'
      - '.cargo/config.toml'
      - '.rustfmt.toml'
      - 'clippy.toml'

jobs:
  code-quality:
    runs-on: ubuntu-22.04
    steps:
      - uses: actions/checkout@v4
        with:
          lfs: true
      - uses: dtolnay/rust-toolchain@master
        with:
          toolchain: # leído de rust-toolchain file
      - uses: Swatinem/rust-cache@v2
      - name: Code quality
        run: source ./.github/scripts/env.sh && source ./.github/scripts/code-quality.sh

  security:
    runs-on: ubuntu-22.04
    continue-on-error: true
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@master
      - uses: Swatinem/rust-cache@v2
      - run: cargo install cargo-audit
      - run: source ./.github/scripts/security.sh
```

### build.yml — detalle (jobs paralelos)

```yaml
strategy:
  matrix:
    include:
      - target: linux-x86_64
        os: ubuntu-22.04
        script: .github/scripts/linux-x86_64.sh
      - target: linux-aarch64
        os: ubuntu-22.04
        script: .github/scripts/linux-aarch64.sh
      - target: windows-x86_64
        os: ubuntu-22.04
        script: .github/scripts/windows-x86_64.sh
      - target: macos-x86_64
        os: macos-latest
        rust_target: x86_64-apple-darwin
      - target: macos-aarch64
        os: macos-latest
        rust_target: aarch64-apple-darwin
```

### publish-docker.yml — detalle

```yaml
- name: Login to GHCR
  uses: docker/login-action@v3
  with:
    registry: ghcr.io
    username: ${{ github.actor }}
    password: ${{ secrets.GITHUB_TOKEN }}

- name: Build and push
  uses: docker/build-push-action@v5
  with:
    context: .
    file: server-cli/Dockerfile
    push: true
    tags: ghcr.io/${{ github.repository_owner }}/veloren/server-cli:${{ env.PUBLISH_DOCKER_TAG }}
```

---

## 2. DB Persistence para Translation y Benchmarks

### Schema (`.github/scripts/db/schema.sql`)

```sql
CREATE TABLE IF NOT EXISTS translations_stage (
  country_code TEXT,
  file_name TEXT,
  translation_key TEXT,
  status TEXT,
  git_commit TEXT
);

CREATE TABLE IF NOT EXISTS translations (
  country_code TEXT,
  file_name TEXT,
  translation_key TEXT,
  status TEXT,
  git_commit TEXT,
  loaded_at TIMESTAMP DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS benchmarks (
  "group" TEXT,
  "function" TEXT,
  value NUMERIC,
  throughput_num NUMERIC,
  throughput_type TEXT,
  sample_measured_value NUMERIC,
  unit TEXT,
  iteration_count INTEGER,
  recorded_at TIMESTAMP DEFAULT NOW(),
  git_commit TEXT,
  branch TEXT
);

CREATE OR REPLACE PROCEDURE public.load_translations_from_stage()
LANGUAGE plpgsql AS $$
BEGIN
  INSERT INTO translations SELECT *, NOW() FROM translations_stage;
  TRUNCATE translations_stage;
END;
$$;
```

### Flujo de persistencia (por cada run de `translation.yml`)

```
1. Service container PostgreSQL levanta
2. Ejecutar schema.sql (crea tablas si no existen)
3. Si ci-data/translations.csv existe: COPY → translations
4. Ejecutar translation.sh (genera translation_analysis.csv)
5. COPY translation_analysis.csv → translations_stage
6. CALL load_translations_from_stage()
7. COPY translations → ci-data/translations.csv
8. git add ci-data/translations.csv && git commit && git push
```

### Variables de entorno para futura migración a servidor propio

Cuando el usuario tenga su propia DB, solo cambia estos GitHub Secrets:
- `DB_HOST` (default: `localhost`)
- `DB_PORT` (default: `5432`)
- `DB_NAME` (default: `veloren_ci`)
- `DB_USER` (default: `postgres`)
- `DB_PASSWORD` (default: `postgres`)
- `DB_SSLMODE` (default: `disable`, → `verify-ca` con servidor propio)
- `DB_SSLROOTCERT` (default: vacío, → path al cert con servidor propio)

---

## 3. Scripts Migrados

### env.sh — cambios

- Eliminar `SHADERC_LIB_DIR="/shaderc/combined/"` (path específico de Docker images del upstream)
- Agregar detección automática de shaderc si está disponible
- Mantener resto igual

### util.sh — cambios

- `publishdockertag()`: reemplazar referencia a `${CI_COMMIT_TAG}`, `${CI_PIPELINE_SOURCE}`, `${CI_DEFAULT_BRANCH}` por equivalentes GitHub Actions:
  - `${GITHUB_REF_TYPE}` + `${GITHUB_REF_NAME}` para tags
  - `${GITHUB_EVENT_NAME}` para schedule
  - `${GITHUB_REF_NAME}` para branch name

---

## 4. Issue Template

`.github/ISSUE_TEMPLATE/bug_report.md` — adaptado de `.gitlab/issue_templates/Bug.md`:
- Reemplazar links de GitLab issues por `https://github.com/mgrinberg/veloren/issues`
- Remover `/label` al final (sintaxis GitLab, no funciona en GitHub)
- Agregar YAML frontmatter de GitHub: `name`, `about`, `labels`

---

## 5. Cleanup de Referencias

### Archivos de código

| Archivo | Línea | Acción |
|---------|-------|--------|
| `world/src/site/mod.rs` | 236 | Eliminar línea con `discord.com` URL |
| `common/state/src/plugin/memory_manager.rs` | 112 | Eliminar línea con `discord.com` URL |
| `server-cli/docker-compose.yml` | 5 | `registry.gitlab.com/veloren/veloren/server-cli:weekly` → `ghcr.io/mgrinberg/veloren/server-cli:weekly` |

### .gitlab-ci.yml

El archivo `.gitlab-ci.yml` puede dejarse en el repo (es el CI del upstream en GitLab). No interfiere con GitHub Actions. Sin embargo, se agrega una nota en el README de que el CI activo para este fork es GitHub Actions.

---

## 6. Secrets necesarios en GitHub

Para configurar en el repo de GitHub (Settings → Secrets):

| Secret | Descripción | Cuándo se necesita |
|--------|-------------|-------------------|
| `GITHUB_TOKEN` | Auto-provisto por GitHub | Siempre |
| `MIRROR_TOKEN_GITHUB` | Ya configurado (mirror.yml) | Ya existe |
| `DB_PASSWORD` | Password de PostgreSQL (default: `postgres`) | Opcional, default suficiente para service container |

---

## 7. Lo que NO cambia

- `.gitlab-ci.yml` — permanece (el upstream lo necesita si se hacen PRs al upstream)
- `.gitlab/` directorio completo — permanece (histórico + para PRs al upstream)
- `mirror.yml` — permanece y funciona (sincroniza desde GitLab upstream a GitHub)
- Todos los archivos de código Rust, excepto las 2 líneas con URLs Discord

---

## Resumen de Archivos a Crear/Modificar

| Archivo | Acción |
|---------|--------|
| `.github/workflows/check.yml` | Crear |
| `.github/workflows/test.yml` | Crear |
| `.github/workflows/build.yml` | Crear |
| `.github/workflows/publish-docker.yml` | Crear |
| `.github/workflows/publish-release.yml` | Crear |
| `.github/workflows/translation.yml` | Crear |
| `.github/workflows/benchmarks.yml` | Crear |
| `.github/workflows/docs.yml` | Crear |
| `.github/scripts/env.sh` | Crear (adaptado) |
| `.github/scripts/code-quality.sh` | Crear (copia) |
| `.github/scripts/security.sh` | Crear (copia) |
| `.github/scripts/unittest.sh` | Crear (copia) |
| `.github/scripts/translation.sh` | Crear (copia) |
| `.github/scripts/benchmark.sh` | Crear (copia) |
| `.github/scripts/coverage.sh` | Crear (copia) |
| `.github/scripts/linux-x86_64.sh` | Crear (copia) |
| `.github/scripts/linux-aarch64.sh` | Crear (copia) |
| `.github/scripts/windows-x86_64.sh` | Crear (copia) |
| `.github/scripts/util.sh` | Crear (adaptado para GitHub) |
| `.github/scripts/plugin.sh` | Crear (copia) |
| `.github/scripts/db/schema.sql` | Crear |
| `.github/scripts/db/import_translations.sql` | Crear |
| `.github/scripts/db/export_translations.sql` | Crear |
| `.github/scripts/db/import_benchmarks.sql` | Crear |
| `.github/scripts/db/export_benchmarks.sql` | Crear |
| `.github/ISSUE_TEMPLATE/bug_report.md` | Crear (adaptado) |
| `ci-data/translations.csv` | Crear (vacío con header) |
| `ci-data/benchmarks.csv` | Crear (vacío con header) |
| `server-cli/docker-compose.yml` | Modificar (URL Docker) |
| `world/src/site/mod.rs` | Modificar (eliminar línea Discord) |
| `common/state/src/plugin/memory_manager.rs` | Modificar (eliminar línea Discord) |
