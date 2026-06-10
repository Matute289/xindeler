# Upstream Merge: gitlab/master → development

**Date:** 2026-06-10
**Branch strategy:** Merge via staging branch, validate with cargo check, fast-forward to development

## Context

Our `development` branch carries 175 commits of custom work (Phase 1–3 terrain pipeline: Transvoxel smooth terrain, block scaling, triplanar normal maps, parallax, logging telemetry system). The upstream `gitlab/master` has accumulated 15 new commits since our divergence point (`4d66a9a65c`).

## Upstream Changes (15 commits)

| Change | Files | Nature |
|---|---|---|
| Fix unreliable entity targeting | `session/target.rs` | Bug fix |
| Fall damage threshold refactor | `entity_manipulation.rs` | Refactor (excess energy calc) |
| Humanoid mass back to ~65kg, less "principled" aerodynamics | `body/mod.rs`, `fluid_dynamics.rs` | Balance |
| Glide physics nerf, fix physics tests | `glide_wield.rs`, `phys/basic.rs` | Balance + test fix |
| Vampire bat knockback nerf | `abilities/vampire/...shockwave_2.ron` | Balance |
| Parry no longer depends on armor | `combat.rs`, 8 ability RON files | Feature change |
| Trade price discount fix in voxygen UI | `hud/mod.rs`, `trade.rs`, `trade_pricing.rs`, `util.rs` | Bug fix |
| Bag/inventory title bar asset fix | 4 PNG files | Art fix |
| Changelog entries | `CHANGELOG.md` | Docs |

## Conflict Analysis

Only 2 files are touched by both upstream and our work:

### `server/src/events/entity_manipulation.rs`
- **Upstream:** Lines ~1457–1471 — refactors fall damage to use `falldmg_threshold` and `excess_energy`
- **Ours:** Lines ~324 and ~1081 — adds telemetry macros for health change and player death events
- **Verdict:** No conflict. Changes are 400+ lines apart in different functions.

### `voxygen/src/hud/mod.rs`
- **Upstream:** Line ~4471 — moves trade price discount calculation outside the `.map()` closure
- **Ours:** Lines 144, 1301, 1444, 3120, 3275, 4005–4068, 5158 — adds `bug_report_status` field and telemetry macros
- **Verdict:** No conflict. Our furthest hunk ends at ~4068; upstream starts at ~4471.

All other upstream files are untouched by our Phase 1–3 work.

## Merge Strategy

**Chosen approach: staging branch with cargo check validation.**

1. Create `upstream-merge-staging` from `development`
2. `git merge gitlab/master` on the staging branch
3. Run `cargo check --workspace` to confirm no compilation errors
4. If clean: merge `upstream-merge-staging` → `development` (fast-forward or squash merge commit)
5. Push to `origin/development`
6. Delete staging branch

## Success Criteria

- `git merge` completes with zero conflict markers
- `cargo check --workspace` exits 0
- Our Phase 1–3 terrain pipeline code is unchanged
- Upstream balance/bug-fix changes are present in `development`
