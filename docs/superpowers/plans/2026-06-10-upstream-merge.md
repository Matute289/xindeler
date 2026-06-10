# Upstream Merge (gitlab/master → development) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring the 15 pending upstream commits from `gitlab/master` into our `development` branch without disrupting Phase 1–3 terrain pipeline work.

**Architecture:** Create a staging branch from `development`, merge `gitlab/master` into it, validate with `cargo check`, then merge the staging branch back into `development`. The staging branch is a quarantine zone — if anything breaks, we delete it with zero impact on `development`.

**Tech Stack:** Git (nightly Rust project), Cargo workspace (`cargo check --workspace`)

---

### Task 1: Verify starting state

**Files:** No file changes — git state verification only.

- [ ] **Step 1: Confirm you are on `development` with a clean working tree**

```bash
git status && git branch --show-current
```
Expected: `On branch development`, `nothing to commit, working tree clean`.
If uncommitted changes exist, stash or commit them before proceeding.

- [ ] **Step 2: Confirm `development` is up to date with `origin/development`**

```bash
git fetch origin && git log --oneline origin/development..development
```
Expected: no output (local development is not ahead of remote).

- [ ] **Step 3: Fetch the latest `gitlab/master`**

```bash
git fetch gitlab
```
Expected: output includes `gitlab/master -> gitlab/master` updated SHA. If already up to date, that's fine too.

- [ ] **Step 4: Confirm divergence counts**

```bash
echo "Upstream commits to bring in:" && git log --oneline development..gitlab/master | wc -l
echo "Our commits upstream doesn't have:" && git log --oneline gitlab/master..development | wc -l
```
Expected: first count ~15, second count ~175.
If first count is 0: nothing to merge — stop here, everything is already in sync.

---

### Task 2: Create staging branch and merge

**Files:** No file changes — git operations only.

- [ ] **Step 1: Create staging branch from `development`**

```bash
git checkout -b upstream-merge-staging
```
Expected: `Switched to a new branch 'upstream-merge-staging'`

- [ ] **Step 2: Attempt the merge**

```bash
git merge gitlab/master --no-edit -m "merge: upstream gitlab/master into development"
```
Expected: merge completes listing changed files, ending with `Merge made by the 'ort' strategy.`
If you see lines starting with `CONFLICT`, the merge stopped — go to **Task 3** before continuing here.
If clean, skip Task 3 and go to Task 4.

- [ ] **Step 3: Confirm no conflict markers remain**

```bash
git diff --check
```
Expected: no output (zero conflict markers in any file).

---

### Task 3: Conflict resolution (skip if Task 2 was clean)

**Context:** Our pre-merge analysis identified 2 files with overlapping edits. Both conflicts are in distinct line regions and should auto-resolve. If you are in this task, something unexpected occurred.

**Files:**
- Modify: `server/src/events/entity_manipulation.rs`
- Modify: `voxygen/src/hud/mod.rs`

- [ ] **Step 1: Identify conflicted files**

```bash
git status | grep "both modified"
```
Note every file listed.

- [ ] **Step 2: Resolve `server/src/events/entity_manipulation.rs` (if conflicted)**

Open the file and find `<<<<<<<` markers.
- **Our side (HEAD):** `common::telemetry!(...)` macros added at lines ~324 (health change event) and ~1081 (player death event).
- **Upstream side (gitlab/master):** Introduces `let falldmg_threshold = 30.0;` and replaces `impact_energy` with `excess_energy` at lines ~1457–1471 (inside `impl ServerEvent for LandOnGroundEvent`).
- **Resolution:** Keep both. The two sets of changes are in completely different functions. Accept our telemetry additions AND upstream's fall-damage formula refactor. Remove all `<<<<<<<`, `=======`, `>>>>>>>` markers.

After editing:
```bash
cargo check -p veloren-server 2>&1 | grep "^error" | head -5
```
Expected: no errors.

- [ ] **Step 3: Resolve `voxygen/src/hud/mod.rs` (if conflicted)**

Open the file and find `<<<<<<<` markers.
- **Our side (HEAD):** `bug_report_status: Option<Arc<Mutex<...>>>` field added to `Hud` struct (~line 1301), `telemetry!` macros in UI event handlers (~lines 3120, 3275, 4005–4068, 5197), `Arc<Mutex>` added to `use std::sync` (~line 144).
- **Upstream side (gitlab/master):** Moves the sell-discount multiplication outside the `.map()` closure at ~line 4471 (trade price display calculation).
- **Resolution:** Keep both. Preserve all our telemetry additions AND apply upstream's discount-outside-map refactor. Both changes are 400+ lines apart.

After editing:
```bash
cargo check -p veloren-voxygen 2>&1 | grep "^error" | head -5
```
Expected: no errors.

- [ ] **Step 4: Stage resolved files and complete the merge**

```bash
git add server/src/events/entity_manipulation.rs voxygen/src/hud/mod.rs
git merge --continue --no-edit
```
Expected: `[upstream-merge-staging <sha>] merge: upstream gitlab/master into development`

---

### Task 4: Validate — cargo check

**Files:** No file changes — compilation check only.

- [ ] **Step 1: Run full workspace check**

```bash
VELOREN_ASSETS="$(pwd)/assets" cargo check --workspace 2>&1 | tail -5
```
Expected: ends with `Finished` line and zero `error[E...]` lines. This takes 1–5 minutes cold.

- [ ] **Step 2: If errors appear, identify their origin**

```bash
VELOREN_ASSETS="$(pwd)/assets" cargo check --workspace 2>&1 | grep "^error" | head -20
```

For each error, look at the file path:
- **`assets/voxygen/shaders/`** or **`voxygen/src/render/`** or **`common/src/terrain/`** → our Phase 1–3 terrain work was unexpectedly affected. Compare the specific line against the upstream diff: `git diff development..upstream-merge-staging -- <file>`.
- **`common/src/comp/fluid_dynamics.rs`**, **`common/src/combat.rs`**, **`common/src/comp/body/mod.rs`** → pure upstream issue unrelated to our work.

Fix the error according to which side introduced it. If the upstream commit broke something in their own code, apply the fix and commit separately.

- [ ] **Step 3: Confirm our Phase 1–3 shader and terrain files are untouched**

```bash
git diff development..upstream-merge-staging -- \
  assets/voxygen/shaders/smooth-terrain-frag.glsl \
  assets/voxygen/shaders/smooth-terrain-vert.glsl \
  voxygen/src/render/pipelines/terrain.rs \
  common/src/terrain/density.rs \
  common/src/terrain/block.rs
```
Expected: no output (none of these files were modified by the upstream merge).

---

### Task 5: Merge staging into development and push

**Files:** No file changes — git operations only.

- [ ] **Step 1: Switch back to `development`**

```bash
git checkout development
```
Expected: `Switched to branch 'development'`

- [ ] **Step 2: Merge the staging branch**

```bash
git merge upstream-merge-staging --no-ff -m "merge: upstream gitlab/master into development"
```
Expected: `Merge made by the 'ort' strategy.` — one merge commit on `development`.

- [ ] **Step 3: Verify the upstream commits are now present**

```bash
git log --oneline | head -20
```
Expected: top of log shows our merge commit, followed by our recent Phase 1–3 commits. The upstream commits (entity targeting fix, fall damage, aerodynamics, parry, trade pricing, bag assets) should be reachable via `git log`.

- [ ] **Step 4: Push to origin**

```bash
git push origin development
```
Expected: `development -> development` accepted. If rejected, check if someone else pushed: `git pull --rebase origin development` first.

- [ ] **Step 5: Delete the staging branch**

```bash
git branch -d upstream-merge-staging
```
Expected: `Deleted branch upstream-merge-staging`.

---

### Task 6: Smoke tests

**Files:** No file changes — validation only.

- [ ] **Step 1: Run common crate tests**

```bash
VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common 2>&1 | tail -5
```
Expected: `test result: ok. N passed; 0 failed`.

- [ ] **Step 2: Clippy check on voxygen publish profile**

```bash
cargo clippy -p veloren-voxygen --locked --no-default-features --features="default-publish" -- -D warnings 2>&1 | tail -5
```
Expected: `Finished` with no `error` or `warning` lines.

- [ ] **Step 3: Confirm upstream physics test passes**

```bash
VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common-systems -- phys 2>&1 | tail -5
```
Expected: `test result: ok` (the upstream aerodynamics/mass rebalance includes an updated physics test; this confirms it passes in our tree too).
