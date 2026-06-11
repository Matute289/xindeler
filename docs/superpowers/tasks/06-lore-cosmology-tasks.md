# Lore & Cosmology (MYTHOS Phase 1) — Task Board

**Source plan:** [../plans/2026-06-11-lore-cosmology.md](../plans/2026-06-11-lore-cosmology.md)
**Execute with:** superpowers:subagent-driven-development, one task per subagent, in plan order.

> Escalation rule: If acceptance fails twice, escalate one model tier and leave a note in the task file.

> Branch setup (before LORE-T1): create `feature/lore-bible` off `development`. All tasks commit to this branch. Invoke the `veloren-lore` skill before writing ANY prose (canon + original-IP rules). Content workflow: worked entries are written inline from the plan; remaining entries go through the `lore-writer` agent (Task tool, `subagent_type: lore-writer`, `.claude/agents/lore-writer.md`) and are **human-curated before commit** — nothing enters `index.ron` uncurated. Do NOT create `_template.md` files (they trip the lint). Frontmatter contract: files in `docs/lore/{10,20,30,40,50}-*/` start with exactly `---` / `id: <canon id>` / `status: canon` / `---`; every id must exist in `assets/lore/index.ron`. IP rule: every proper noun is original — check the denylist (LORE-T2) and `grep -ri <name> docs/lore assets` before introducing any new name.

> Scope note: readable book items (`ItemKind::Book`) are Phase 2 — OUT of this board.

## LORE-T1 — Scaffold `docs/lore/` and seed `00-cosmology.md`

- **Model:** fable — canon root authoring: the skeleton is given but every `[copy ...]` directive requires transcribing spec §4 tables verbatim while expanding prose in the canon voice (lore creative authoring per routing policy).
- **Depends on:** none.
- **Branch / commit:** `feature/lore-bible` — `lore: scaffold docs/lore and seed cosmology canon root`
- **Files:**
  - Create: `docs/lore/00-cosmology.md`; dirs `docs/lore/10-pantheon/_severed/`, `10-pantheon/_ascended/`, `20-fiends/`, `30-outer-gods/`, `40-planes/`, `50-history/`, `60-npcs/` (empty until Phase 3)
  - Modify: none
  - Delete: none
- **Assets:** `00-cosmology.md` text — Claude (fable) creates inline: plan skeleton + tables transcribed verbatim from spec §4.2–§4.5 (`docs/superpowers/specs/2026-06-10-lore-cosmology-design.md`). Ids and names are FIXED; only prose may expand.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 1' steps 1–3 verbatim. Required sections: The Unsounded / The Worldsong / The Three Tiers of Divinity (12 Primes + 5 Severed tables, Ascended roster) / Fiends: War-Debris (both arch-fiend tables) / The Planes (11-row table) / The Veil / Retcon Rules for Existing Game Content (§4.5 verbatim).
- **Acceptance:**
  - `grep -c "deity\." docs/lore/00-cosmology.md` → ≥ 12.
  - `grep -c "plane\." docs/lore/00-cosmology.md` → ≥ 11.
- **Size:** M

## LORE-T2 — `70-style-guide.md`: phonology + forbidden-names denylist

- **Model:** haiku — the full file (phonology table, tone rules, 28-token denylist) is given verbatim in the plan; pure transcription with one machine-contract trap.
- **Depends on:** LORE-T1 (directory exists).
- **Branch / commit:** `feature/lore-bible` — `lore: style guide with per-culture phonology and forbidden-names denylist`
- **Files:**
  - Create: `docs/lore/70-style-guide.md`
  - Modify: none
  - Delete: none
- **Assets:** style-guide text — Claude creates inline (full text in plan).
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 2' steps 1–2 verbatim. MACHINE CONTRACT: the fenced block tag must be literally ```` ```denylist ```` — LORE-T10's lint parses it. "dagon" is deliberately NOT on the denylist (upstream creature name, retconned not removed) — do not "fix" that.
- **Acceptance:**
  - `awk '/```denylist/,/^```$/' docs/lore/70-style-guide.md | grep -vc '```'` → 28.
- **Size:** S

## LORE-T3 — Pantheon: the 12 Prime Chorus deities

- **Model:** fable — canon creative authoring: 2 worked entries transcribed + 10 dispatched to the `lore-writer` agent with the plan's exact prompt, then a human-curation editorial pass (fable per routing policy, executed via lore-writer).
- **Depends on:** LORE-T1 (canon facts source), LORE-T2 (phonology + denylist the prompt enforces).
- **Branch / commit:** `feature/lore-bible` — `lore: 12 Prime Chorus deity entries`
- **Files:**
  - Create: `docs/lore/10-pantheon/solenne.md`, `nereth.md` (worked, full text in plan); via lore-writer: `veshtur.md`, `yssira.md`, `maravel.md`, `verdessa.md`, `toldram.md`, `hestrel.md`, `lunere.md`, `pell.md`, `gildmar.md`, `velora.md`
  - Modify: none
  - Delete: none
- **Assets:** all 12 markdown entries — Claude/fable creates (2 inline from plan, 10 via lore-writer dispatches; human-curate before commit).
- **Downloads/tools:** `lore-writer` agent — 10 independent dispatches (parallelize), using the plan's Step 2 prompt verbatim with `<file>`, `<Name>`, `<id>` substituted.
- **Steps:** Follow plan section '### Task 3' steps 1–3 verbatim. Constraints baked into the prompt: frontmatter exact, section order (Verse / Faith / Relations / In Velor today / closing Voice line), max 1 page, canon facts from the Prime Chorus table unchanged, relations only as backticked canon ids from `00-cosmology.md`, every new proper noun passes High Eleth phonology + denylist + collision grep.
- **Acceptance:**
  - `ls docs/lore/10-pantheon/*.md | wc -l` → 12.
  - `for f in docs/lore/10-pantheon/*.md; do sed -n '2p' "$f" | grep -q '^id: deity\.' || echo "BAD FRONTMATTER: $f"; done` → no output.
- **Size:** L

## LORE-T4 — The Severed and the Ascended

- **Model:** fable — canon creative authoring via lore-writer + curation (1 worked + 6 dispatched), with per-entity canon constraints to enforce.
- **Depends on:** LORE-T3 (entry template + voice established; Severed reference Primes).
- **Branch / commit:** `feature/lore-bible` — `lore: Severed and Ascended entries`
- **Files:**
  - Create: `docs/lore/10-pantheon/_severed/drazkhul.md` (worked, full text in plan); via lore-writer: `_severed/{vukarra,szorvenn,ghorvul,kelzhara}.md`, `_ascended/{velken,auressa}.md`
  - Modify: none
  - Delete: none
- **Assets:** all 7 markdown entries — Claude/fable creates (1 inline, 6 via lore-writer; human-curate before commit).
- **Downloads/tools:** `lore-writer` agent — 6 dispatches with the Task 3 prompt adjusted per plan Step 2.
- **Steps:** Follow plan section '### Task 4' steps 1–3 verbatim. Severed entries (ids `deity_dark.*`) use drazkhul's sections (The Severing / Cult & Servants / In Velor today) and the Severed phonology register; Szorvenn carries the extra canon facts listed in the plan (backwards verses, treacherous pacts, pirates "whispered to the Hollow", WANTS worship unlike the outer tier). Ascended (`ascended.velken`, `ascended.auressa`) use the Prime sections and keep the Velken/Auressa canon facts (ember-lantern through the Long Dark; gliders are "Auressa's wings", `GliderCourse` sites her shrine-trials).
- **Acceptance:**
  - `ls docs/lore/10-pantheon/_severed/*.md | wc -l` → 5.
  - `ls docs/lore/10-pantheon/_ascended/*.md | wc -l` → 2.
- **Size:** M

## LORE-T5 — Arch-fiends: Iron Courts and the Churn

- **Model:** fable — canon creative authoring via lore-writer + curation (1 worked + 3 dispatched), two phonology registers.
- **Depends on:** LORE-T3 (template/voice), LORE-T1 (fiend tables).
- **Branch / commit:** `feature/lore-bible` — `lore: arch-fiend entries (Iron Courts, Churn)`
- **Files:**
  - Create: `docs/lore/20-fiends/malverant.md` (worked, full text in plan); via lore-writer: `serqitel.md`, `uzghorath.md`, `vyshka.md`
  - Modify: none
  - Delete: none
- **Assets:** all 4 markdown entries — Claude/fable creates (1 inline, 3 via lore-writer; human-curate before commit).
- **Downloads/tools:** `lore-writer` agent — 3 dispatches per plan Step 2.
- **Steps:** Follow plan section '### Task 5' steps 1–3 verbatim. Registers: "Fiends — devils" for Serqitel (`fiend.serqitel`), "Fiends — demons" for Uzghorath (`fiend.uzghorath`) and Vyshka (`fiend.vyshka`). Canon rule for demon entries: demons cannot contract, only consume; they generate ORACLE combat-event content (horde incursions), never deals.
- **Acceptance:**
  - `ls docs/lore/20-fiends/*.md | wc -l` → 4.
- **Size:** M

## LORE-T6 — Outer gods: Quolzeth and Ulgrethu

- **Model:** fable — the hardest tonal register in the canon (facts-about-witnesses-only cosmic horror); 1 worked + 1 dispatched + curation.
- **Depends on:** LORE-T3 (template/voice), LORE-T2 (tone rules are mandatory here).
- **Branch / commit:** `feature/lore-bible` — `lore: outer-god entries (Quolzeth, Ulgrethu)`
- **Files:**
  - Create: `docs/lore/30-outer-gods/quolzeth.md` (worked, full text in plan); via lore-writer: `ulgrethu.md`
  - Modify: none
  - Delete: none
- **Assets:** both markdown entries — Claude/fable creates (1 inline, 1 via lore-writer; human-curate before commit).
- **Downloads/tools:** `lore-writer` agent — 1 dispatch per plan Step 2.
- **Steps:** Follow plan section '### Task 6' steps 1–3 verbatim. Ulgrethu (`outer.ulgrethu`) canon facts: "Ulgrethu, the Thought That Eats"; devourer of names and minds, anti-memory; Mindflayer boss is an avatar-fragment, not a species; the Cult of the Eaten Name (the game's Cultists) are mortals hollowed out by it; opposes `deity.lunere`'s dreams. Canon contract from quolzeth.md: Veil corruption comes ONLY from the Unsounded, never from gods or fiends.
- **Acceptance:**
  - `ls docs/lore/30-outer-gods/*.md | wc -l` → 2.
- **Size:** S

## LORE-T7 — The 11 planes

- **Model:** fable — canon creative authoring via lore-writer + curation (1 worked + 10 dispatched) with cross-file consistency constraints.
- **Depends on:** LORE-T5 (ironcourts/churn must stay consistent with fiend entries), LORE-T6 (`unsounded.md` follows the outer-tier rules), LORE-T1 (plane table).
- **Branch / commit:** `feature/lore-bible` — `lore: 11 plane entries`
- **Files:**
  - Create: `docs/lore/40-planes/gleam.md` (worked, full text in plan); via lore-writer: `velor.md`, `gloam.md`, `cinderdeep.md`, `everbrine.md`, `skyvault.md`, `adamant.md`, `meridian.md`, `ironcourts.md`, `churn.md`, `unsounded.md`
  - Modify: none
  - Delete: none
- **Assets:** all 11 markdown entries — Claude/fable creates (1 inline, 10 via lore-writer; human-curate before commit).
- **Downloads/tools:** `lore-writer` agent — 10 dispatches per plan Step 2.
- **Steps:** Follow plan section '### Task 7' steps 1–3 verbatim. Sections: Harmonic / Inhabitants & Hazards / Playability / Voice line. Canon facts (id, nature, playable-via) come from the `00-cosmology.md` plane table — keep all three unchanged. Special constraints: `unsounded.md` is never explained, never fully playable ("thin places" only); `meridian.md` is sealed, opens only during ORACLE arc events; `ironcourts.md`/`churn.md` consistent with `20-fiends/` (seven tiered courts; ever-dissolving abyss).
- **Acceptance:**
  - `ls docs/lore/40-planes/*.md | wc -l` → 11.
- **Size:** L

## LORE-T8 — The 6 era chronicles

- **Model:** fable — canon creative authoring via lore-writer + curation (1 worked + 5 dispatched), including the deliberately under-documented `era.unsounded`.
- **Depends on:** LORE-T4 (Velken canon reused in longdark), LORE-T1, LORE-T2 (era.unsounded rule).
- **Branch / commit:** `feature/lore-bible` — `lore: 6 era chronicles`
- **Files:**
  - Create: `docs/lore/50-history/longdark.md` (worked, full text in plan); via lore-writer: `unsounded.md`, `worldsong.md`, `accord.md`, `severance.md`, `embers.md`
  - Modify: none
  - Delete: none
- **Assets:** all 6 markdown entries — Claude/fable creates (1 inline, 5 via lore-writer; human-curate before commit).
- **Downloads/tools:** `lore-writer` agent — 5 dispatches per plan Step 2.
- **Steps:** Follow plan section '### Task 8' steps 1–3 verbatim. Sections: Order header / Chronicle / one named-focus section / Scars in the world / Voice line. Constraints: `era.unsounded` (order 1) under one page of deliberate non-information; `era.accord` (order 3) names Haniwa, Myrmidon, Terracotta, dwarven delves; `era.severance` (order 4) left the map's ruins and calcified the fiends; `era.embers` (order 6, current) frames every ORACLE arc as "a crack in the Veil".
- **Acceptance:**
  - `ls docs/lore/50-history/*.md | wc -l` → 6.
- **Size:** M

## LORE-T9 — Machine-readable canon: `assets/lore/index.ron` + `common/src/lore.rs`

- **Model:** haiku — the entire loader (types, `load`, `validate`), the test, the lib.rs registration, and the FULL index.ron are given verbatim; mechanical TDD transcription via the existing `AssetExt`/`Ron` pattern.
- **Depends on:** LORE-T1 (ids are canon); content tasks T3–T8 precede it in plan order so the index matches shipped entries (the test itself only needs the RON).
- **Branch / commit:** `feature/lore-bible` — `feat: machine-readable lore canon index with serde loader and validation`
- **Files:**
  - Create: `assets/lore/index.ron` (new top-level asset dir beside `assets/common/`; specifier `lore.index`), `common/src/lore.rs`
  - Modify: `common/src/lib.rs` (`pub mod lore;` alphabetically between `pub mod lod;` and `pub mod lottery;`, ~line 53)
  - Delete: none
- **Assets:** `assets/lore/index.ron` — RON config, Claude creates inline (full 19-deity/4-fiend/2-outer/11-plane/6-era text in plan). NOTE: `i18n_key` values are FORWARD references — `lore.ftl` ships in Phase 2; do NOT wire them into i18n completeness tests or "fix" the dangling keys.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 9' steps 1–5 verbatim. TDD: test-only module first ("cannot find `LoreIndex`"), then types/loader, then the RON. Counts asserted: Prime 12 / Severed 5 / Ascended 2 / fiends 4 / outer 2 / planes 11 / eras 6. If RON parsing fails, fix the asset, not the schema.
- **Acceptance:**
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common lore -- --nocapture` → `lore_index_loads_and_validates ... ok`.
- **Size:** M

## LORE-T10 — Canon lint: frontmatter ids and denylist as Rust tests

- **Model:** haiku — both lint tests are given verbatim, including the path-resolution helpers; the only judgment is executing the prescribed negative check and reverting.
- **Depends on:** LORE-T9 (`LoreIndex` + tests module), LORE-T2 (denylist block parsed), LORE-T1…T8 (the docs the lint walks).
- **Branch / commit:** `feature/lore-bible` — `test: canon lint — frontmatter id cross-check and forbidden-names denylist`
- **Files:**
  - Create: none
  - Modify: `common/src/lore.rs` (extend the Task 9 `tests` module)
  - Delete: none
- **Assets:** none.
- **Downloads/tools:** none. (Docs reached via `CARGO_MANIFEST_DIR/../docs/lore`; supersedes the spec's `lore-lint.sh` — plain `cargo test` means CI gets it free.)
- **Steps:** Follow plan section '### Task 10' steps 1–3 verbatim. DO NOT SKIP the negative check: temporarily set `id: deity.typo` in `solenne.md` and add `cthulhu` to `gleam.md`, rerun, expect BOTH tests to fail naming file and offender; revert and confirm green before committing.
- **Acceptance:**
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common lore -- --nocapture` → 3 tests PASS.
  - Negative check demonstrated (both failure modes caught, then reverted to green).
- **Size:** M

## LORE-T11 — In-game surfacing wave 1: loading tips + villager lore lines

- **Model:** haiku — all 8 .ftl lines and the Deferred-section markdown are given verbatim; both Fluent keys pick random attributes at runtime, so ZERO code changes.
- **Depends on:** LORE-T1 (lines reference canon: Dawnmother, Severance War, vampire courts), LORE-T2 (Deferred section appended to the style guide).
- **Branch / commit:** `feature/lore-bible` — `feat: first in-game lore strings — loading tips and villager small-talk`
- **Files:**
  - Create: none
  - Modify: `assets/voxygen/i18n/en/main.ftl` (append `.b0`–`.b4` to `loading-tips`, after `.a21` ~line 121), `assets/voxygen/i18n/en/npc.ftl` (append `.a8`–`.a10` to `npc-speech-villager_open`, after `.a7` ~line 12), `docs/lore/70-style-guide.md` (Deferred to Phase 2 section)
  - Delete: none
- **Assets:** 5 loading tips + 3 villager lines (.ftl) — Claude creates inline (full text in plan; `.b` series marks lore tips). Non-EN languages never roll the new variants until translated — existing localization tests treat missing non-EN attributes as warnings.
- **Downloads/tools:** optional `veloren-run` eyeball of the connecting screen.
- **Steps:** Follow plan section '### Task 11' steps 1–5 verbatim. Step 4 records the gated follow-ups ONLY (no code): per-culture NameGen data, `lore.ftl` + i18n_key references, `ItemKind::Book` + lore dialogue topic — all Phase 2.
- **Acceptance:**
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-client-i18n` → PASS (validates .ftl syntax).
- **Size:** S

## LORE-T12 — Lint, format, changelog, finish

- **Model:** haiku — mechanical CI-identical commands and a verbatim changelog entry; `lore.rs` is the only new Rust surface.
- **Depends on:** LORE-T1 … LORE-T11.
- **Branch / commit:** `feature/lore-bible` — `docs: changelog entry for lore canon Phase 1` (+ any fix commits)
- **Files:**
  - Create: none
  - Modify: `CHANGELOG.md` (under `### Added` in `## [Unreleased]`, ~line 15; + whatever clippy/fmt fixes touch)
  - Delete: none
- **Assets:** none.
- **Downloads/tools:** `superpowers:finishing-a-development-branch` + `veloren-review` before merging into `development`.
- **Steps:** Follow plan section '### Task 12' steps 1–5 verbatim. Fix clippy warnings in `lore.rs` properly — no `#[allow]` without a justifying comment. RON and markdown are untouched by rustfmt.
- **Acceptance:**
  - `cargo clippy --all-targets --locked --features="bin_cmd_doc_gen,bin_compression,bin_csv,bin_graphviz,bin_bot,bin_asset_migrate,asset_tweak,bin,stat,cli" -- -D warnings` → clean.
  - `cargo fmt --all -- --check` → clean.
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common -p veloren-client-i18n` → PASS, including the 3 lore tests.
- **Size:** S
