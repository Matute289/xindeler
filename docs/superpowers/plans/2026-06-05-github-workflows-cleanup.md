# GitHub Workflows Cleanup

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop all failing GitHub Actions runs in our fork and fix the YAML syntax error in `upstream-sync.yml`.

**Architecture:**
- **Root cause 1 — Upstream workflows:** 8 CI workflow files were mirrored from the Veloren GitLab upstream. They trigger on every push/PR and immediately fail because they need GitLab CI secrets, Docker registries, and PostgreSQL infrastructure our fork doesn't have. They also consume GitHub Actions free minutes, which has hit the account limit.
- **Root cause 2 — YAML syntax error:** `upstream-sync.yml` uses `$(cat <<'EOF' ... EOF)` inside a YAML `run:` block. The YAML parser chokes on the heredoc inside command substitution. Fix: move the PR body to a YAML `env:` variable and pass it with `--body "$PR_BODY"`.
- **Root cause 3 — Billing:** GitHub Actions free minutes were exhausted (the account billing/spending limit needs attention — this is outside the scope of code changes).

**Our 4 custom workflows to KEEP:**
| File | Purpose |
|------|---------|
| `check-source-branch.yml` | Blocks PRs to main not from development/hot-fix |
| `mirror.yml` | Manual-only GitLab→GitHub mirror |
| `sync-hotfix.yml` | Auto-resets hot-fix branch after merge to main |
| `upstream-sync.yml` | Manual upstream review with draft PR |

**8 upstream Veloren workflows to DELETE:**
| File | Why delete |
|------|-----------|
| `benchmarks.yml` | Requires PostgreSQL CI database, GitLab runners |
| `build.yml` | Requires GitLab Docker build infrastructure |
| `check.yml` | Requires GitLab CI runners and cache |
| `docs.yml` | Requires GitLab Pages deployment token |
| `publish-docker.yml` | Requires Docker Hub credentials |
| `publish-release.yml` | Requires GitLab release infrastructure |
| `test.yml` | Requires GitLab CI runners |
| `translation.yml` | Requires Weblate/i18n service tokens |

**⚠️ USER ACTION REQUIRED:** Check GitHub billing at github.com/settings/billing — the account has hit its Actions free-tier spending limit. Add a payment method or increase the spending limit so that our 4 lightweight custom workflows can run.

---

## Task 1: Delete upstream Veloren CI workflows

- [ ] **Step 1: Delete the 8 upstream workflow files**

```bash
cd /Users/mgrinberg/Workspace/RustroverProjects/veloren
git rm .github/workflows/benchmarks.yml \
       .github/workflows/build.yml \
       .github/workflows/check.yml \
       .github/workflows/docs.yml \
       .github/workflows/publish-docker.yml \
       .github/workflows/publish-release.yml \
       .github/workflows/test.yml \
       .github/workflows/translation.yml
```

- [ ] **Step 2: Verify only 4 workflows remain**

```bash
ls .github/workflows/
```
Expected output (4 files only):
```
check-source-branch.yml
mirror.yml
sync-hotfix.yml
upstream-sync.yml
```

- [ ] **Step 3: Commit the deletions**

```bash
git commit -m "chore(ci): remove upstream Veloren CI workflows not applicable to this fork

These workflows (build, check, test, docs, benchmarks, publish-docker,
publish-release, translation) were mirrored from veloren/veloren and require
GitLab CI infrastructure, Docker registries, and PostgreSQL that this private
fork doesn't have. They trigger on every push and consume GitHub Actions
minutes without benefit."
```

---

## Task 2: Fix `upstream-sync.yml` YAML syntax error

The current `upstream-sync.yml` uses `$(cat <<'EOF' ... EOF)` inside a YAML `run:` block. YAML parses this file before the shell sees it, and the heredoc syntax inside a `$()` command substitution causes a parse error at line 72.

**Fix:** Move the PR body to a YAML `env:` variable (proper multiline YAML string), then pass it as `--body "$PR_BODY"` in the shell command.

- [ ] **Step 4: Replace the "Create draft PR for review" step**

The current step (lines 55-109) looks like:
```yaml
      - name: Create draft PR for review
        if: steps.check.outputs.has_new_commits == 'true'
        env:
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
        run: |
          ...
          gh pr create \
            ...
            --body "$(cat <<'EOF'
          ## Upstream changes to review
          ...
          EOF
          )"
```

Replace the entire step with the version below that uses a YAML `env:` variable for the body:

```yaml
      - name: Create draft PR for review
        if: steps.check.outputs.has_new_commits == 'true'
        env:
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
          PR_BODY: |
            ## Upstream changes to review

            This is an automated **draft PR** for manual review of new commits from the Veloren GitLab upstream.

            **DO NOT merge this PR automatically.** A human must:

            1. Review the diff
            2. Identify which upstream changes are safe to integrate (bug fixes, asset updates, non-conflicting features)
            3. Identify which upstream changes conflict with our custom modifications (terrain rendering, Transvoxel, collision, etc.)
            4. For each conflict: adapt the upstream change to work alongside our code, or discard it if it reverts our work
            5. Only merge after conflicts are resolved and changes are validated

            ## Our priority areas (never let upstream overwrite these)

            - `voxygen/src/mesh/terrain.rs` — Transvoxel meshing path
            - `voxygen/src/mesh/transvoxel.rs` — Transvoxel algorithm
            - `voxygen/src/render/pipelines/smooth_terrain.rs` — smooth pipeline
            - `assets/voxygen/shaders/smooth-terrain-*.glsl` — smooth shaders
            - `common/src/terrain/density.rs` — density field
            - `common/systems/src/phys_smooth.rs` — smooth collision
            - `voxygen/src/settings/graphics.rs` — TerrainSmoothingMode
            - `docs/superpowers/` — our internal docs

            ## How to resolve conflicts

            ```bash
            # 1. Pull the review branch locally
            git fetch origin
            git checkout upstream/review-YYYY-MM-DD

            # 2. Create a working integration branch
            git checkout -b upstream/integrate-YYYY-MM-DD origin/main

            # 3. Merge and resolve conflicts manually, keeping our changes
            git merge upstream/review-YYYY-MM-DD

            # 4. Push integration branch and update this PR
            ```
        run: |
          UPSTREAM_SHORT="${{ steps.check.outputs.upstream_short }}"
          DATE=$(date +%Y-%m-%d)
          BRANCH="${{ steps.branch.outputs.branch }}"

          gh pr create \
            --base main \
            --head "$BRANCH" \
            --draft \
            --title "⚠️ [Upstream Review] veloren/veloren@${UPSTREAM_SHORT} — ${DATE}" \
            --body "$PR_BODY"
```

- [ ] **Step 5: Validate the YAML syntax**

```bash
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/upstream-sync.yml'))" && echo "YAML valid" || echo "YAML INVALID"
```

Expected: `YAML valid`

- [ ] **Step 6: Commit the fix**

```bash
git add .github/workflows/upstream-sync.yml
git commit -m "fix(ci): fix YAML syntax error in upstream-sync.yml

Replace heredoc-in-command-substitution pattern with a YAML env variable
for the PR body. The old pattern caused a YAML parse error at line 72."
```

---

## Task 3: Push to GitHub and verify

- [ ] **Step 7: Push to development**

```bash
TOKEN=$(gh auth token) && git push "https://x-token:${TOKEN}@github.com/Matute289/veloren.git" development
```

- [ ] **Step 8: Create and merge PR to main**

```bash
gh pr create --repo Matute289/veloren --base main --head development \
  --title "chore(ci): remove upstream workflows + fix YAML syntax error" \
  --body "$(printf '## Summary\n\n- Delete 8 upstream Veloren CI workflows that fail without GitLab infrastructure and consume GitHub Actions minutes\n- Fix YAML syntax error in upstream-sync.yml (heredoc in run block → env variable)\n\n## User action required\n\nCheck GitHub billing settings — the account has hit the free-tier Actions spending limit.')"
gh pr merge --repo Matute289/veloren --number 6 --merge
```

- [ ] **Step 9: Verify on GitHub**

```bash
gh run list --repo Matute289/veloren --limit 5
```

After the next push/PR, only these workflows should appear:
- `Check source branch` ✓
- `Sync hot-fix after merge to main` ✓
- (mirror and upstream-sync are manual-only, won't appear here)

No more: Build, Check, Test, Docs, Benchmarks, Publish Docker, Publish Release, Translation

---

## User action required (outside this plan)

**GitHub billing:** Go to https://github.com/settings/billing and either:
- Add a payment method and set a spending limit (e.g. $5/month)
- Or check if your free minutes have reset (they reset monthly)

Our 4 remaining lightweight workflows are fast (<30 seconds each) and should stay within the free tier after the heavy upstream workflows are removed.
