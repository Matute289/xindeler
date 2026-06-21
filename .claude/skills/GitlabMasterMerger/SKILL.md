# GitlabMasterMerger

Merges `gitlab/master` (upstream Veloren) into the current working branch safely, after a thorough conflict analysis that accounts for **our** custom objectives documented in specs, plans, and tasks.

## When to use

- User says "ponete la skill GitlabMasterMerger" or "ejecutá GitlabMasterMerger"
- Any time you need to bring upstream Veloren changes into our fork

---

## Execution Model

**Announce at start:** "Running GitlabMasterMerger skill — fetching upstream and analyzing conflicts."

Use **Opus** (most capable model) for the conflict evaluation phase (Phase 2). The analysis must be thorough — a wrong judgment here can corrupt weeks of work. Efficiency elsewhere is fine.

---

## Phase 1: Gather state (run all commands in parallel where possible)

```bash
# 1a. Fetch both remotes
git fetch gitlab && git fetch origin

# 1b. Show current branch
git branch --show-current

# 1c. Count divergence
echo "=== Upstream commits we lack ===" && git log --oneline HEAD..gitlab/master
echo "=== Our commits upstream lacks ===" && git log --oneline gitlab/master..HEAD | wc -l

# 1d. Find merge base
git merge-base HEAD gitlab/master
```

If `git log --oneline HEAD..gitlab/master` is empty → **stop and report** "Nothing to merge — already in sync."

```bash
# 1e. List files upstream changed (relative to merge base)
MERGE_BASE=$(git merge-base HEAD gitlab/master)
git diff --name-only $MERGE_BASE gitlab/master

# 1f. List files WE changed (relative to same merge base)
git diff --name-only $MERGE_BASE HEAD

# 1g. Find the overlap
UPSTREAM_FILES=$(git diff --name-only $MERGE_BASE gitlab/master)
OUR_FILES=$(git diff --name-only $MERGE_BASE HEAD)
comm -12 <(echo "$UPSTREAM_FILES" | sort) <(echo "$OUR_FILES" | sort)
```

```bash
# 1h. Read our documented objectives — specs, plans, tasks
ls docs/design/specs/ 2>/dev/null
ls docs/design/plans/ 2>/dev/null
# Read the most recent spec and any open plans
ls -t docs/design/specs/*.md 2>/dev/null | head -3
ls -t docs/design/plans/*.md 2>/dev/null | head -5
```

Read the content of the most recent spec file and the last 3 plan files to understand current objectives.

---

## Phase 2: Conflict evaluation (use Opus — be thorough)

For each overlapping file, run:

```bash
MERGE_BASE=$(git merge-base HEAD gitlab/master)

# What upstream changed in this file
git diff $MERGE_BASE gitlab/master -- <FILE>

# What we changed in this file
git diff $MERGE_BASE HEAD -- <FILE>
```

Then evaluate each overlap on **three axes**:

### A. Line proximity
- **No overlap**: Changes are in clearly separate functions/regions (100+ lines apart). Git auto-merges. ✅ Safe.
- **Close but distinct**: Changes are in the same function but different blocks. Manual review needed. ⚠️ Caution.
- **Same lines**: Both sides edited the same lines. Hard conflict. 🔴 Must resolve manually.

### B. Semantic impact on our objectives
Cross-reference the file against our specs and plans:

- **Unrelated to our objectives**: Upstream changed a file we only touched for telemetry/logging/infra. We keep both. ✅ Safe.
- **Touches our feature area**: The upstream change modifies code in a subsystem we built (e.g., the magic / attunement systems, combat formulas, abilities, classes/races, progression). **Evaluate carefully**: does the upstream change conflict with our design goals? Read the relevant spec section.
- **Overwrites our work**: The upstream change replaces code we implemented. 🔴 Critical — see Decision Matrix below.

### C. Value of the upstream change
- Is this a bug fix in an unrelated system? (high value, merge it)
- Is this a balance tweak in an unrelated system? (medium value, merge it)
- Is this a refactor in code we own? (evaluate — maybe adapt ours to align)
- Is this a feature that conflicts with our feature? (evaluate carefully)

### Decision Matrix

| Line proximity | Semantic impact | Decision |
|---|---|---|
| No overlap | Any | **Auto-merge** — proceed directly to Phase 3 |
| Close/same lines | Unrelated to our objectives | **Manual merge** — keep both, Phase 3 with conflict resolution |
| Close/same lines | Touches our feature area | **Adaptation needed** — Phase 3 + invoke `superpowers:writing-plans` |
| Same lines | Overwrites our work | **Full evaluation** — see below |

### Full evaluation for overwriting conflicts

When upstream overwrites something we intentionally built, produce a structured report:

```
FILE: <path>
UPSTREAM CHANGE: <describe what they changed in 1-2 sentences>
OUR CHANGE: <describe what we built and why, citing the relevant spec>
CONFLICT TYPE: Overwrite / Semantic / Structural
IMPACT IF WE SKIP UPSTREAM: <what we lose by not taking their change>
IMPACT IF WE TAKE UPSTREAM: <what our feature loses or gains>
ADAPTATION REQUIRED: <yes/no — if yes, describe what code changes are needed>
RECOMMENDATION: Skip upstream change | Take upstream + adapt ours | Take upstream as-is
```

If **adaptation is required**, after the merge invoke:
```
superpowers:writing-plans
```
with a brief describing which files need adaptation and what the target behavior is.

---

## Phase 3: Execute merge

### 3a. Create staging branch

```bash
git checkout -b upstream-merge-staging
```

### 3b. Merge

```bash
git merge gitlab/master --no-edit -m "merge: upstream gitlab/master ($(git log --oneline HEAD..gitlab/master | wc -l | tr -d ' ') commits)"
```

### 3c. If conflicts arose — resolve them

For each conflicted file from Phase 2:

```bash
git status | grep "both modified"
```

Apply the resolution strategy determined in Phase 2 for each file:
- **Keep both**: Accept our changes AND upstream changes — place them both in the file without conflict markers.
- **Keep ours**: Remove upstream's version of conflicting lines, keep ours intact.
- **Keep upstream + adapt**: Apply upstream change, then adjust our surrounding code to remain compatible (document in a separate adaptation plan via `writing-plans`).

After resolving each file:
```bash
git add <file>
```

Then:
```bash
git merge --continue --no-edit
```

### 3d. Verify no conflict markers remain

```bash
git diff --check
```
Expected: no output. If output appears, open each listed file and remove remaining `<<<<<<<` / `=======` / `>>>>>>>` markers.

---

## Phase 4: Validate

```bash
# Full workspace compilation check
VELOREN_ASSETS="$(pwd)/assets" cargo check --workspace 2>&1 | tail -10
```

If errors appear:
```bash
VELOREN_ASSETS="$(pwd)/assets" cargo check --workspace 2>&1 | grep "^error" | head -30
```

For each error:
1. Identify the crate and file.
2. Run `git log --oneline --follow <file> | head -5` to see if it was touched by the merge.
3. If the error is in a file we modified: check if upstream's change broke our API or vice versa.
4. Fix the error. If it requires non-trivial code changes, invoke `superpowers:writing-plans` to plan the adaptation before touching code.

Run again after fixing:
```bash
VELOREN_ASSETS="$(pwd)/assets" cargo check --workspace 2>&1 | tail -5
```
Must end with `Finished`. Do not proceed to Phase 5 until this passes.

Also confirm our key feature files kept our intent through the merge:
```bash
git diff upstream-merge-staging..upstream-merge-staging~1 -- \
  common/src/comp/attunement.rs \
  server/src/sys/attunement.rs \
  common/src/combat.rs \
  common/src/comp/ability.rs \
  common/src/comp/inventory/
```
Expected: these files should not appear in the merge diff unless upstream explicitly changed them (which is rare and would require deep evaluation in Phase 2).

---

## Phase 5: Integrate and push

```bash
# Switch to working branch
git checkout <original-branch>   # e.g., development

# Merge staging (preserve merge commit for traceability)
git merge upstream-merge-staging --no-ff -m "merge: upstream gitlab/master into $(git branch --show-current)"

# Push
git push origin $(git branch --show-current)

# Clean up staging
git branch -d upstream-merge-staging
```

---

## Phase 6: Smoke tests

```bash
# Common crate unit tests
VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common 2>&1 | tail -5

# Physics tests (upstream often updates these with balance changes)
VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common-systems -- phys 2>&1 | tail -5

# Voxygen clippy (publish profile, no hot-reload)
cargo clippy -p veloren-voxygen --locked --no-default-features --features="default-publish" -- -D warnings 2>&1 | tail -5
```

All three must pass. If a test fails:
- Check if the upstream commits changed expected values in the test.
- Check `git log --oneline upstream-merge-staging -- <test-file>` to see if upstream touched it.
- Fix or update the test accordingly.

---

## Phase 7: Report

After a successful merge, produce a concise summary:

```
## Upstream Merge Complete

**Branch:** <branch>
**Commits merged:** <N> from gitlab/master
**Conflicts encountered:** <none | list of files>
**Resolution strategy:** <auto | manual keep-both | adaptation required>
**cargo check:** ✅ passed
**Tests:** ✅ passed

### Upstream changes now in our branch:
- <bullet per commit category>

### Our Phase 1–3 work: unaffected ✅
```

If adaptation plans were created, list them:
```
### Follow-up plans created:
- docs/design/plans/<filename>.md — <one line description>
```

---

## Key files to always protect (our active work — magic / RPG / attunement)

Never accept upstream changes to these without explicit user confirmation:

- **Attunement (ENG-D2):** `common/src/comp/attunement.rs`, `server/src/sys/attunement.rs`,
  and the `RequiresAttunement` / `has_tag` / `requires_attunement` additions in
  `common/src/comp/inventory/mod.rs`.
- **Magic / combat / abilities:** `common/src/combat.rs`, `common/src/comp/ability.rs`,
  the spell taxonomy / `SpellDef` files, `common/systems/src/{beam,melee,arcing,pool,shockwave,projectile,buff,stats,character_behavior}.rs`,
  `common/src/states/behavior.rs` + `utils.rs`.
- **Items / progression:** `common/src/comp/inventory/`, skillset / character levels /
  classes-races, `server/src/persistence/*` (⚠️ DB schema).
- **CI / LFS / privacy:** `.lfsconfig`, `.gitattributes`, `.github/workflows/*` — **never
  re-introduce GitHub LFS** (keep VPS-SSH LFS); upstream's `.gitlab-ci.yml` / `.gitlab/CI/*`
  are theirs (we run GitHub Actions).
- `docs/design/` — our internal design repo.

(The old smooth-terrain / Transvoxel pipeline was **discarded** — see
`docs/design/DEFERRED-TO-V2.md`. Do not protect or re-introduce it.)

If upstream touches any of these, escalate to full evaluation in Phase 2 before proceeding.