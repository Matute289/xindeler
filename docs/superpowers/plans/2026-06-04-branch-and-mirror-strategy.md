# Feature Branch Extraction + Upstream Mirror Strategy

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Isolate all new terrain work onto a feature branch, disable the destructive auto-mirror from GitLab, replace it with a manual upstream review workflow, then land the feature through PR → development → main.

**Architecture:** The 26 local commits on `development` (not yet pushed to origin) get published as `feat/smooth-terrain-pipeline`. The auto-mirror that force-pushes gitlab/master onto `main` is disabled by removing its cron trigger. A new `upstream-sync.yml` fetches upstream on demand, creates a review branch, and opens a draft PR — a human decides what to integrate, always prioritizing our changes. After the mirror fix is live, we create and merge two PRs: feature → development, then development → main.

**Tech Stack:** Git, GitHub Actions (YAML), `gh` CLI for PR operations, HTTPS push via `gh auth token`.

---

## Current State

- Local `development`: `a9f7f48573` (26 commits ahead of `origin/development`)
- `origin/development`: `bb02b5e11b` (stable base — no local commits pushed yet)
- `origin/main`: `63c5a81e6c`
- `origin` push: must use HTTPS with GH token (`TOKEN=$(gh auth token) && git push "https://x-token:${TOKEN}@github.com/Matute289/valoren.git" <branch>`)

---

## File Map

| File | Change |
|------|--------|
| `.github/workflows/mirror.yml` | Remove `schedule:` trigger — keeps `workflow_dispatch:` only |
| `.github/workflows/upstream-sync.yml` | New: manual upstream review workflow |

---

## Task 1: Stage the plan doc and create the feature branch

**Files:** git operations only

- [ ] **Step 1.1: Commit the plan doc that's currently untracked**

```bash
cd /Users/mgrinberg/Workspace/RustroverProjects/veloren
git add docs/superpowers/plans/2026-06-04-fase1-smooth-terrain-pipeline.md \
        docs/superpowers/plans/2026-06-04-branch-and-mirror-strategy.md
git commit -m "docs: add smooth terrain pipeline plan and branch strategy plan"
```

- [ ] **Step 1.2: Create the feature branch at current HEAD**

```bash
git checkout -b feat/smooth-terrain-pipeline
```

Verify:
```bash
git log --oneline origin/development..HEAD | wc -l
```
Expected: `27` (the 26 original commits + the plan doc commit above).

- [ ] **Step 1.3: Push the feature branch to origin via HTTPS**

```bash
TOKEN=$(gh auth token)
git push "https://x-token:${TOKEN}@github.com/Matute289/valoren.git" feat/smooth-terrain-pipeline
```

Expected: branch is now visible at `https://github.com/Matute289/valoren/tree/feat/smooth-terrain-pipeline`.

- [ ] **Step 1.4: Switch back to development**

```bash
git checkout development
```

`development` stays at the same HEAD (we just created a branch from it, not moved it).

- [ ] **Step 1.5: Verify origin/development is untouched**

```bash
git log --oneline origin/development..HEAD | wc -l
```
Expected: `27` (same as before — local development is still 27 ahead, origin/development not modified).

---

## Task 2: Disable the automatic mirror

The current `mirror.yml` runs every hour and force-pushes GitLab master onto `main`. This destroys any custom commits we make to `main` and can overwrite our features.

**Files:**
- Modify: `.github/workflows/mirror.yml`

- [ ] **Step 2.1: Read the current file**

```bash
cat .github/workflows/mirror.yml
```

- [ ] **Step 2.2: Remove the cron schedule**

Replace the `on:` block in `.github/workflows/mirror.yml`. Change from:

```yaml
on:
  schedule:
    - cron: "0 * * * *"
  workflow_dispatch:
```

To:

```yaml
on:
  workflow_dispatch:
```

Also update the job name for clarity. Final full content of `mirror.yml`:

```yaml
name: Mirror from GitLab upstream (manual only)

on:
  workflow_dispatch:

jobs:
  mirror:
    runs-on: ubuntu-latest
    permissions:
      contents: write

    steps:
      - name: Install Git LFS
        run: |
          sudo apt-get install -y git-lfs
          git lfs install

      - name: Mirror GitLab master → GitHub main
        env:
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
        run: |
          GITHUB_URL="https://x-access-token:${GH_TOKEN}@github.com/${{ github.repository }}.git"
          git clone --bare --branch master https://gitlab.com/veloren/veloren.git repo.git
          cd repo.git
          git lfs fetch --all
          git remote add github "${GITHUB_URL}"
          git lfs push --all github
          git push --force github "+refs/heads/master:refs/heads/main"
          git push --force github --tags || echo "Some upstream tags skipped (workflow permission restriction)"
```

- [ ] **Step 2.3: Commit**

```bash
git add .github/workflows/mirror.yml
git commit -m "ci: disable automatic hourly mirror from GitLab upstream"
```

---

## Task 3: Create manual upstream review workflow

This workflow fetches from GitLab, creates a dated review branch, and opens a **draft PR** against `main`. A human reviews the diff, decides what to integrate, and handles conflicts manually — always prioritizing our changes.

**Files:**
- Create: `.github/workflows/upstream-sync.yml`

- [ ] **Step 3.1: Create the workflow file**

```yaml
# .github/workflows/upstream-sync.yml
name: Upstream Sync Review

# Run manually when we want to check for upstream changes.
# This NEVER auto-merges — it creates a draft PR for human review.
on:
  workflow_dispatch:

jobs:
  sync-review:
    runs-on: ubuntu-latest
    permissions:
      contents: write
      pull-requests: write

    steps:
      - name: Checkout main
        uses: actions/checkout@v4
        with:
          ref: main
          fetch-depth: 0
          token: ${{ secrets.GITHUB_TOKEN }}

      - name: Fetch GitLab upstream
        run: |
          git remote add gitlab https://gitlab.com/veloren/veloren.git
          git fetch gitlab master --no-tags

      - name: Check for new upstream commits
        id: check
        run: |
          UPSTREAM_SHA=$(git rev-parse gitlab/master)
          MAIN_SHA=$(git rev-parse origin/main)
          echo "upstream_sha=$UPSTREAM_SHA" >> $GITHUB_OUTPUT
          echo "upstream_short=${UPSTREAM_SHA:0:8}" >> $GITHUB_OUTPUT

          if git merge-base --is-ancestor "$UPSTREAM_SHA" "$MAIN_SHA"; then
            echo "has_new_commits=false" >> $GITHUB_OUTPUT
            echo "No new upstream commits — main is already up to date."
          else
            COUNT=$(git rev-list "$MAIN_SHA..gitlab/master" --count)
            echo "has_new_commits=true" >> $GITHUB_OUTPUT
            echo "Found $COUNT new upstream commits to review."
          fi

      - name: Create review branch
        if: steps.check.outputs.has_new_commits == 'true'
        id: branch
        run: |
          DATE=$(date +%Y-%m-%d)
          BRANCH="upstream/review-${DATE}"
          git checkout -b "$BRANCH" gitlab/master
          git push origin "$BRANCH"
          echo "branch=$BRANCH" >> $GITHUB_OUTPUT

      - name: Create draft PR for review
        if: steps.check.outputs.has_new_commits == 'true'
        env:
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
        run: |
          UPSTREAM_SHORT="${{ steps.check.outputs.upstream_short }}"
          DATE=$(date +%Y-%m-%d)
          BRANCH="${{ steps.branch.outputs.branch }}"

          gh pr create \
            --base main \
            --head "$BRANCH" \
            --draft \
            --title "⚠️ [Upstream Review] veloren/veloren@${UPSTREAM_SHORT} — ${DATE}" \
            --body "$(cat <<'EOF'
## Upstream changes to review

This is an automated **draft PR** created for manual review of new commits from the Veloren GitLab upstream.

**DO NOT merge this PR automatically.** A human must:

1. Review the diff below
2. Identify which upstream changes are **safe to integrate** (bug fixes, asset updates, non-conflicting features)
3. Identify which upstream changes **conflict with our custom modifications** (terrain rendering, Transvoxel, collision, etc.)
4. For each conflict: adapt the upstream change to work alongside our code, or discard it if it reverts our work
5. Only merge after conflicts are resolved and changes are validated

## Our priority areas (do not let upstream overwrite these)

- `voxygen/src/mesh/terrain.rs` — Transvoxel meshing path
- `voxygen/src/mesh/transvoxel.rs` — Transvoxel algorithm
- `voxygen/src/render/pipelines/smooth_terrain.rs` — smooth pipeline
- `assets/voxygen/shaders/smooth-terrain-*.glsl` — smooth shaders
- `common/src/terrain/density.rs` — density field
- `common-systems/src/phys_smooth.rs` — smooth collision
- `voxygen/src/settings/graphics.rs` — TerrainSmoothingMode
- `docs/superpowers/` — our internal docs

## How to resolve conflicts

```bash
# 1. Pull the review branch locally
git fetch origin
git checkout upstream/review-YYYY-MM-DD

# 2. Create a working integration branch
git checkout -b upstream/integrate-YYYY-MM-DD origin/main

# 3. Cherry-pick upstream commits selectively
#    (or merge and resolve conflicts manually)
git merge upstream/review-YYYY-MM-DD

# 4. For each conflict: keep our version, adapt upstream logic if needed
# 5. Push integration branch and update this PR's head
```
EOF
)"

      - name: Report no new commits
        if: steps.check.outputs.has_new_commits == 'false'
        run: echo "✅ No new upstream commits. main is already up to date with GitLab master."
```

- [ ] **Step 3.2: Commit**

```bash
git add .github/workflows/upstream-sync.yml
git commit -m "ci: add manual upstream review workflow (replaces auto-mirror)"
```

---

## Task 4: Push feature branch updates and create PR feat → development

After Tasks 2-3 are done on the `feat/smooth-terrain-pipeline` branch (or on `development` first — see note below), we need to push everything and create the first PR.

**Note on where to make changes:** Tasks 2 and 3 can be done directly on `feat/smooth-terrain-pipeline` — the mirror fix ships as part of the feature PR. This is intentional: the mirror fix and feature land together in one PR to `development`, then `development` → `main` brings both to production.

- [ ] **Step 4.1: Ensure you're on the feature branch**

```bash
git checkout feat/smooth-terrain-pipeline
git log --oneline | head -5
```

The HEAD should include the mirror-fix commits from Tasks 2-3.

- [ ] **Step 4.2: Push updated feature branch**

```bash
TOKEN=$(gh auth token)
git push "https://x-token:${TOKEN}@github.com/Matute289/valoren.git" feat/smooth-terrain-pipeline --force-with-lease
```

- [ ] **Step 4.3: Create PR feat → development**

```bash
gh pr create \
  --base development \
  --head feat/smooth-terrain-pipeline \
  --title "feat: Fase 1 SmoothTerrainVertex pipeline + disable auto-mirror" \
  --body "$(cat <<'EOF'
## Summary

- **Smooth terrain rendering**: New `SmoothTerrainPipeline` for Transvoxel meshes with float positions, 10-10-10-2 packed normals, and per-vertex color. Eliminates faceted appearance from integer-quantized vertices.
- **GLSL shaders**: `smooth-terrain-vert.glsl` + `smooth-terrain-frag.glsl` — full PBR lighting pipeline, per-vertex color decoded without atlas texture.
- **Mesh generation**: `generate_mesh` Transvoxel path now emits `Mesh<SmoothTerrainVertex>` instead of `Mesh<TerrainVertex>`; greedy path unaffected.
- **Scene wiring**: `Terrain::render_smooth()` + call in first render pass.
- **Mirror strategy**: Disabled automatic hourly mirror from GitLab upstream. Added `upstream-sync.yml` for manual reviewed sync with draft PRs.

## Components (in merge order)

- `voxygen/src/render/pipelines/smooth_terrain.rs` — vertex format + pipeline
- `assets/voxygen/shaders/smooth-terrain-*.glsl` — vertex + fragment shaders
- `voxygen/src/render/renderer/drawer.rs` — SmoothTerrainDrawer
- `voxygen/src/mesh/terrain.rs` — Transvoxel → SmoothTerrainVertex
- `voxygen/src/scene/terrain/mod.rs` — TerrainChunkData + render_smooth
- `.github/workflows/mirror.yml` — cron removed, manual only
- `.github/workflows/upstream-sync.yml` — new manual upstream review workflow

## Test Plan

- [ ] `cargo clippy --all-targets` — clean
- [ ] `cargo clippy -p veloren-voxygen --no-default-features --features=default-publish` — clean
- [ ] `cargo fmt --all -- --check` — clean
- [ ] Launch game with `TerrainSmoothingMode::Soft` → terrain appears smooth
- [ ] Launch game with `TerrainSmoothingMode::Disabled` → greedy terrain unchanged
EOF
)"
```

- [ ] **Step 4.4: Merge the PR**

```bash
gh pr merge --merge --delete-branch
```

(Use `--merge` for a merge commit, preserving history. If the CI checks block the merge, use `gh pr merge --admin` or merge manually on GitHub.)

---

## Task 5: Create and merge PR development → main

- [ ] **Step 5.1: Update local development**

```bash
TOKEN=$(gh auth token)
git fetch "https://x-token:${TOKEN}@github.com/Matute289/valoren.git" development
git checkout development
git pull
```

- [ ] **Step 5.2: Create PR development → main**

```bash
gh pr create \
  --base main \
  --head development \
  --title "release: Fase 1 SmoothTerrainVertex pipeline" \
  --body "$(cat <<'EOF'
## Summary

Merges `development` into `main`. Includes all Fase 1 smooth terrain work:

- Transvoxel algorithm + density field
- TerrainSmoothingMode settings (Disabled/Soft/Smooth/Ultra)
- SmoothTerrainPipeline with float positions + 10-10-10-2 normals
- Per-vertex color baked from column atlas
- Full PBR lighting in GLSL shaders
- Smooth collision skeleton (phys_smooth)
- Disabled destructive auto-mirror from GitLab upstream

## Verification

All CI checks must pass. See feat PR for full test plan.
EOF
)"
```

- [ ] **Step 5.3: Verify CI passes, then merge**

```bash
gh pr checks --watch   # wait for CI
gh pr merge --merge
```

- [ ] **Step 5.4: Tag the release**

```bash
git fetch origin
git checkout main
git pull
git tag -a "v0.1-smooth-terrain" -m "Fase 1: SmoothTerrainVertex pipeline complete"
TOKEN=$(gh auth token)
git push "https://x-token:${TOKEN}@github.com/Matute289/valoren.git" "v0.1-smooth-terrain"
```

---

## Scope Check

| Requirement | Covered by |
|-------------|-----------|
| Move smooth terrain work to a feature branch | Task 1 |
| Disable automatic hourly GitLab → main mirror | Task 2 |
| Manual upstream review workflow with draft PR | Task 3 |
| Upstream integration instructions | Task 3 (PR body) |
| PR feat → development | Task 4 |
| PR development → main | Task 5 |
