# Lore & Cosmology (MYTHOS Phase 1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

> **Scope note:** Readable book items are a **verified gap** (no `ItemKind::Book` in `common/src/comp/inventory/item/mod.rs`) and belong to Phase 2 of the spec — OUT of this plan. This plan is Phase 1: Lore Bible, machine index + loader, canon lint, and the cheapest in-game surfacing (loading tips, villager small-talk; site naming explicitly gated).

**Goal:** A curated original canon ("Project MYTHOS") under `docs/lore/` — Worldsong creation myth, 12 Prime deities, 5 Severed, 2 Ascended, 4 arch-fiends, 2 outer gods, 11 planes, 6 eras — plus `assets/lore/index.ron` loaded by a new `common/src/lore.rs`, Rust canon-lint tests (frontmatter ids + forbidden-names denylist), and first lore strings live in-game.

**Architecture:** Prose canon is markdown in `docs/lore/` with frontmatter carrying a canon id. The machine index is one versioned RON asset at `assets/lore/index.ron` (new top-level dir beside `assets/common/` — verified peers `common/`, `server/`, `voxygen/`, `world/`; specifier `lore.index`), deserialized by plain serde structs in `common/src/lore.rs` (no ECS coupling) via the existing `crate::assets::{AssetExt, Ron}` pattern (exemplar: `test_all_skillset_assets`, `common/src/skillset_builder.rs:158`). Canon lint lives as `#[test]`s in the same module, reaching docs via `CARGO_MANIFEST_DIR/../docs/lore` (verified: `common/` is a direct child of the workspace root). In-game strings ride existing Fluent pipelines: `loading-tips` (`assets/voxygen/i18n/en/main.ftl:95`, consumed by `voxygen/src/menu/main/ui/connecting.rs:115` via `get_variation_ctx`) and `npc-speech-villager_open` (`assets/voxygen/i18n/en/npc.ftl:5`, consumed by `common/src/rtsim.rs:196` via `PersonalityTrait::Open`) — both pick a random attribute, so new attributes need **zero code changes**.

**Tech Stack:** Markdown + RON + Fluent (.ftl); Rust nightly (2024 edition) for loader/lint. Design spec: `docs/superpowers/specs/2026-06-10-lore-cosmology-design.md`. Content agent: `.claude/agents/lore-writer.md`.

**Conventions for every task:**
- Run tests with the assets path: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p <crate>`
- Branch: create `feature/lore-bible` off `development` before Task 1.
- Invoke the `veloren-lore` skill before writing any prose; it enforces canon + IP rules.
- **Content workflow:** worked entries are written inline here (canon seed, adapted from the spec). Remaining entries: dispatch the `lore-writer` agent (Task tool, `subagent_type: lore-writer`) with the exact prompt in the step, then **human-curate before commit** — nothing enters `index.ron` uncurated. Worked entries ARE the template; do NOT create `_template.md` files (they would trip the lint).
- **Frontmatter contract (lint, defined once):** every file in `docs/lore/{10,20,30,40,50}-*/` starts with exactly `---` / `id: <canon id>` / `status: canon` / `---`. `00-cosmology.md` and `70-style-guide.md` carry no id. Every `id:` must exist in `assets/lore/index.ron` (Task 10 enforces).
- IP rule: every proper noun is original. Check the denylist (Task 2) and `grep -ri <name> docs/lore assets` before introducing any new name.

---

### Task 1: Scaffold `docs/lore/` and seed `00-cosmology.md`

**Files:**
- Create: `docs/lore/00-cosmology.md`; dirs `10-pantheon/_severed/`, `10-pantheon/_ascended/`, `20-fiends/`, `30-outer-gods/`, `40-planes/`, `50-history/`, `60-npcs/` (empty until Phase 3)

- [ ] **Step 1: Create the tree**

```bash
mkdir -p docs/lore/10-pantheon/_severed docs/lore/10-pantheon/_ascended \
  docs/lore/20-fiends docs/lore/30-outer-gods docs/lore/40-planes \
  docs/lore/50-history docs/lore/60-npcs
```

- [ ] **Step 2: Write `docs/lore/00-cosmology.md`**

This is the spec's §4 promoted to canon. Write the file with exactly these sections; `[copy ...]` means transcribe that table from the spec verbatim — ids and names are fixed, prose may expand:

```markdown
# Cosmology of Velor

> Canon root. Machine ids: `assets/lore/index.ron`. Style rules: `70-style-guide.md`.

## The Unsounded
Before everything: the **Unsounded** — not darkness, not void, but *that which the Song
never named*. Wherever the Song is thin — deep caves, drowned trenches, the spaces behind
mirrors — the Unsounded leaks back in. (This sentence is the lore engine for every horror
hook in the game.)

## The Worldsong
Creation was a chorus: primal voices — the gods-to-be — sang reality into structure. Each
verse fixed a law of the world: stone, tide, flame, breath, growth, ending. The world is
named **Velor** in High Eleth, "the sung place"; the calendar goddess **Velora, the Wheel
of Years** is its namesake and keeper.

## The Three Tiers of Divinity
1. **The Prime Chorus (12)** — the original singers; post-Sealing they act only through
   domains, omens, and clergy. [copy the 12-row table from spec §4.2]
2. **The Severed (5)** — Chorus members who tried to rewrite the Song in their own voice
   during the Severance War and were cast out. [copy the 5-row table from §4.2]
3. **The Ascended (open roster)** — mortals raised into the Song's margins, each tied to
   one Prime and one game mechanic: Saint Velken the Lantern-Bearer (`ascended.velken`,
   under Solenne), Auressa the Skyborn (`ascended.auressa`, under Maravel).

## Fiends: War-Debris
Fiends are not gods; they calcified from the Severance War's spilled power. Two phyla:
**the Iron Courts** (lawful devils — cannot lie outright, only contract) and **the Churn**
(chaotic demons — cannot contract, only consume). [copy both arch-fiend tables from §4.3]

## The Planes
[copy the 11-row plane table from §4.4: velor, gleam, gloam, cinderdeep, everbrine,
skyvault, adamant, meridian, ironcourts, churn, unsounded]

## The Veil
To end the Severance War, the Chorus sealed itself (and the Severed) behind the **Veil**;
gods can no longer walk Velor. The present age (era.embers) is the Veil thinning: cult
resurgence, monsters stirring, thin places opening.

## Retcon Rules for Existing Game Content
[copy the §4.5 table verbatim: Cultists → Cult of the Eaten Name (outer.ulgrethu);
Mindflayer → avatar-fragment of Ulgrethu; Dagon → sailors' species-name for spawn of
outer.quolzeth, never a god; Gnarlings → feral Verdessa splinter-faith;
Haniwa/Terracotta/Myrmidon → Severance-War empires; VampireCastle → court of Drazkhul]
```

- [ ] **Step 3: Verify and commit**

```bash
grep -c "deity\." docs/lore/00-cosmology.md   # Expected: >= 12
grep -c "plane\." docs/lore/00-cosmology.md   # Expected: >= 11
git add docs/lore && git commit -m "lore: scaffold docs/lore and seed cosmology canon root"
```

---

### Task 2: `70-style-guide.md` — phonology + forbidden-names denylist

**Files:**
- Create: `docs/lore/70-style-guide.md`

- [ ] **Step 1: Write the style guide**

The `denylist` fenced block is a **machine contract** — Task 10's lint parses it; keep the fence tag literally ```` ```denylist ````:

```markdown
# Naming & Style Guide

## Per-Culture Phonology
| Register | Phonology rule | Examples |
|---|---|---|
| Prime deities (High Eleth) | Open syllables, long vowels, liquids (l, r, n); feminine `-a/-e`, masculine `-ur/-am` | Solenne, Maravel, Toldram |
| The Severed | One harsh cluster (kh, zk, gh, vk) + closed final syllable | Drazkhul, Vukarra |
| Fiends — devils | Latinate, legal-sounding, 3 syllables | Malverant, Serqitel |
| Fiends — demons | Guttural, doubled consonants | Uzghorath, Vyshka |
| Outer tier | Deliberately awkward clusters; always framed as approximations | Quolzeth, Ulgrethu |
| Settlements | Existing `NameGen` syllables (`world/src/site/namegen.rs`); per-culture sets are Phase 2 data | — |

## Tone Rules
- Outer-tier names are approximations; flavor text must frame them so ("Quolzeth — a
  sailor's stammer, not a name"). Cults that "worship" the outer tier are wrong.
- Cosmic horror implies, never explains. Divine lore is told through worshippers, never
  omniscient narration. Every text states its in-world author, era, and reliability in a
  closing Voice line; unreliable narrators encouraged for outer-gods material.
- 1-page cap per entity file in Phase 1.
- `era.unsounded` is deliberately under-documented: no precise lore may be written about
  it. Ever.

## Frontmatter Contract
Files in numbered subdirs carry `id:` + `status: canon`; ids must resolve in
`assets/lore/index.ron` (enforced by `cargo test -p veloren-common lore`).

## Forbidden Names (machine-checked denylist)
Any name from Exandria/Critical Role, Forgotten Realms, Greyhawk, or the Lovecraft mythos
is banned even as homage. One lowercase token per line; the lint tokenizes every file
under docs/lore/ and fails on any match:

```denylist
cthulhu
azathoth
nyarlathotep
sothoth
shub-niggurath
hastur
tsathoggua
rlyeh
pelor
melora
ioun
avandra
erathis
sehanine
corellon
moradin
bahamut
tiamat
asmodeus
vecna
tharizdun
lolth
gruumsh
mystra
faerun
exandria
wildemount
greyhawk
```
```

(Note: "dagon" is deliberately NOT on the denylist — it exists upstream as a creature name and is retconned, not removed.)

- [ ] **Step 2: Verify and commit**

```bash
awk '/```denylist/,/^```$/' docs/lore/70-style-guide.md | grep -vc '```'   # Expected: 28
git add docs/lore/70-style-guide.md
git commit -m "lore: style guide with per-culture phonology and forbidden-names denylist"
```

---

### Task 3: Pantheon — the 12 Prime Chorus deities

**Files:**
- Create: `docs/lore/10-pantheon/solenne.md`, `nereth.md` (worked, below)
- Create via lore-writer: `veshtur.md`, `yssira.md`, `maravel.md`, `verdessa.md`, `toldram.md`, `hestrel.md`, `lunere.md`, `pell.md`, `gildmar.md`, `velora.md`

- [ ] **Step 1: Write the two worked entries**

`docs/lore/10-pantheon/solenne.md`:

```markdown
---
id: deity.solenne
status: canon
---
# Solenne, the Dawnmother

**Tier:** Prime Chorus · **Domains:** Light, Sun, Healing, Law · **Disposition:** Lawful good

## Verse
First voice of the Worldsong; she sang the verse of light, and the first thing light did
was show the other singers each other. Law, in her liturgy, is "light agreed upon".

## Faith
Iconography: an unlidded lantern. Her clergy heal, keep hospices, and hunt undeath — not
out of hatred, but because undeath is theft from her ally Nereth. Folk religion: every
lit lantern is "a small dawn", a wordless prayer to Solenne.

## Relations
Allies: `deity.nereth`. Enemies: `deity_dark.drazkhul`, `outer.ulgrethu`. Ascended saint:
`ascended.velken`.

## In Velor today
The game's ubiquitous lanterns are her folk-rite. Her faith opposes the vampire courts
(`SiteKind::VampireCastle`, courts of Drazkhul).

> *Voice: hospice catechism of the Lantern Hall, era.embers, reliable.*
```

`docs/lore/10-pantheon/nereth.md`:

```markdown
---
id: deity.nereth
status: canon
---
# Nereth, the Quiet Door

**Tier:** Prime Chorus · **Domains:** Death, Memory, Fate · **Disposition:** True neutral

## Verse
She sang the final verse — that all songs end. Not evil; the gentlest of the Chorus.
Memory is her mercy: nothing that ended is unsung, only finished.

## Faith
Her clergy run funerals, keep grave-registries, and sit with the dying. They abhor
undeath above all: a corpse that walks is a door held open by force.

## Relations
Allies: `deity.solenne` (light and death allied — inverting the cliché). Enemies:
`deity_dark.drazkhul`, who tore her verse to keep his court alive forever.

## In Velor today
Graveyards are her ground. AURORA religions of Nereth run funerals and oppose
`deity_dark.drazkhul` cults wherever vampire castles stand.

> *Voice: a grave-registrar's preface, era.embers, reliable.*
```

- [ ] **Step 2: Dispatch lore-writer for the remaining 10 Primes**

Dispatch the `lore-writer` agent once per deity (10 independent dispatches — parallelize), substituting `<file>`, `<Name>`, `<id>`:

> Write `docs/lore/10-pantheon/<file>.md` for <Name> (canon id `<id>`). First read `docs/lore/00-cosmology.md`, `docs/lore/70-style-guide.md`, and the finished entries `docs/lore/10-pantheon/solenne.md` and `nereth.md` — match their frontmatter exactly (`id: <id>` / `status: canon`), section order (Verse / Faith / Relations / In Velor today / closing Voice line), length (max 1 page), and voice. Canon facts you must keep unchanged: the deity's name, epithet, domains, disposition, and existing-content hook from the Prime Chorus table in `00-cosmology.md`. Relations may only reference canon ids appearing in `00-cosmology.md` tables, formatted as backticked ids. Every new proper noun must pass the High Eleth phonology and the denylist; grep `docs/lore` and `assets/` for collisions before using it.

- [ ] **Step 3: Curate, verify, commit**

Human editorial pass on all 10 generated files (canon rule). Then:

```bash
ls docs/lore/10-pantheon/*.md | wc -l    # Expected: 12
for f in docs/lore/10-pantheon/*.md; do
  sed -n '2p' "$f" | grep -q '^id: deity\.' || echo "BAD FRONTMATTER: $f"
done                                      # Expected: no output
git add docs/lore/10-pantheon && git commit -m "lore: 12 Prime Chorus deity entries"
```

---

### Task 4: The Severed and the Ascended

**Files:**
- Create: `docs/lore/10-pantheon/_severed/drazkhul.md` (worked, below)
- Create via lore-writer: `_severed/{vukarra,szorvenn,ghorvul,kelzhara}.md`; `_ascended/{velken,auressa}.md`

- [ ] **Step 1: Write the worked entry**

`docs/lore/10-pantheon/_severed/drazkhul.md`:

```markdown
---
id: deity_dark.drazkhul
status: canon
---
# Drazkhul, the Sleepless Crown

**Tier:** Severed · **Corrupted domain:** Undeath, tyranny (Nereth's door, forced open)

## The Severing
A king among the singers who refused his own ending. He tore Nereth's verse and sang it
backwards over his court, so that none of them would ever finish. The Chorus cast his
voice out of the Song; what remains of it is the cold that pools in crypts.

## Cult & Servants
His "crowned" are undead lords sworn to the Sleepless Crown; his cults promise their
patrons exactly what he kept — continuation without life.

## In Velor today
`SiteKind::VampireCastle` sites are courts of Drazkhul; Harvester-style undead bosses are
his crowned servants. AURORA may spawn Drazkhul cults near vampire castles. Opposed by:
`deity.nereth`, `deity.solenne`.

> *Voice: deposition of a captured court herald, era.embers, hostile witness — partly lies.*
```

- [ ] **Step 2: Dispatch lore-writer for 4 Severed + 2 Ascended**

Six dispatches with the Task 3 prompt, adjusted: `_severed/` files (ids `deity_dark.vukarra`, `.szorvenn`, `.ghorvul`, `.kelzhara`) match `drazkhul.md`'s sections (The Severing / Cult & Servants / In Velor today) and the "Severed" phonology register; canon facts from the Severed table in `00-cosmology.md`, plus for Szorvenn: *steals verses and sings them backwards into mortal ears; patron of treacherous pacts; pirates who break Gildmar's contracts "whispered to the Hollow"; wants worship — unlike the outer tier, which does not notice its cults*. The `_ascended/` files (ids `ascended.velken`, `ascended.auressa`) use the Prime sections instead and must keep: Velken — miner who carried the first ember-lantern through the Long Dark, patron of everyone who presses the lantern key, under Solenne; Auressa — first mortal to ride the storm-winds, gliders and airships are "Auressa's wings", `GliderCourse` sites are her shrine-trials, under Maravel.

- [ ] **Step 3: Curate, verify, commit**

```bash
ls docs/lore/10-pantheon/_severed/*.md | wc -l    # Expected: 5
ls docs/lore/10-pantheon/_ascended/*.md | wc -l   # Expected: 2
git add docs/lore/10-pantheon && git commit -m "lore: Severed and Ascended entries"
```

---

### Task 5: Arch-fiends — Iron Courts and the Churn

**Files:**
- Create: `docs/lore/20-fiends/malverant.md` (worked, below)
- Create via lore-writer: `serqitel.md`, `uzghorath.md`, `vyshka.md`

- [ ] **Step 1: Write the worked entry**

`docs/lore/20-fiends/malverant.md`:

```markdown
---
id: fiend.malverant
status: canon
---
# Malverant, Lord of the First Ledger

**Phylum:** Arch-devil (the Iron Courts) · **Portfolio:** Debt, soul-contracts, usury

## Calcification
Not born, not sung: calcified out of the Severance War's spilled power, where a god's
broken oath soaked into the ground. Devils are bound by the letter of the Song —
Malverant cannot lie outright, only contract.

## The First Ledger
Every bargain he signs is true, exact, and ruinous. His clerks audit souls the way
Gildmar's clergy audit coin — which is why the Open Hand's temples are the one place his
contracts can be contested.

## In Velor today
Design rule: devils generate quest/dialogue content — deals, escape clauses, with Gildmar
clergy as the counter-faction. Hierarchies below the arch tier are Phase 3.

> *Voice: marginalia in a Gildmar temple case-ledger, era.embers, reliable but partisan.*
```

- [ ] **Step 2: Dispatch lore-writer for the remaining 3**

Three dispatches, Task 3 prompt adjusted: target `docs/lore/20-fiends/<file>.md`, match `malverant.md`'s sections; phonology register "Fiends — devils" for Serqitel (`fiend.serqitel`: The Gilded Judge — corrupt law, perjured oaths, tribunals), "Fiends — demons" for Uzghorath (`fiend.uzghorath`: The Maw Unending — gluttony, hordes, devouring) and Vyshka (`fiend.vyshka`: Queen of the Howling Waste — storms of ruin, frenzy, stampede). Demon entries keep the canon rule: demons cannot contract, only consume; they generate ORACLE combat-event content (horde incursions), never deals.

- [ ] **Step 3: Curate, verify, commit**

```bash
ls docs/lore/20-fiends/*.md | wc -l   # Expected: 4
git add docs/lore/20-fiends && git commit -m "lore: arch-fiend entries (Iron Courts, Churn)"
```

---

### Task 6: Outer gods — Quolzeth and Ulgrethu

**Files:**
- Create: `docs/lore/30-outer-gods/quolzeth.md` (worked, below)
- Create via lore-writer: `ulgrethu.md`

- [ ] **Step 1: Write the worked entry**

`docs/lore/30-outer-gods/quolzeth.md`:

```markdown
---
id: outer.quolzeth
status: canon
---
# Quolzeth (a sailor's stammer, not a name)

**Tier:** The Unsounded · **Aspect:** Drowned immensity; pressure, brine, patience

## What can be said
Nothing here is a fact about Quolzeth; these are facts about what people near it become.
It has no domains, no alignment, no worship-contract. The drowned cults think they
worship it. They are wrong, in the way a barnacle is wrong about the ship.

## Tidemarks
Sailors call its spawn "dagons" — a species-name, never a god's. The Sea Chapel is a
drowned thin place above it. Sahagin sorcerers are half-changed thralls who got too close.

## In Velor today
Veil corruption (canon contract: corruption comes ONLY from the Unsounded, never from
gods or fiends) emanates from its thin places. Buff mechanics belong to the magic spec.

> *Voice: assembled from three logbooks, none of which agree, era.embers, unreliable.*
```

- [ ] **Step 2: Dispatch lore-writer for Ulgrethu**

One dispatch, Task 3 prompt adjusted: target `docs/lore/30-outer-gods/ulgrethu.md`, id `outer.ulgrethu`, matching `quolzeth.md`'s sections and its *facts-about-witnesses-only* framing (style-guide tone rules mandatory). Canon facts: designation "Ulgrethu, the Thought That Eats"; aspect: devourer of names and minds, anti-memory; the Mindflayer boss is an avatar-fragment ("a thought it left behind"), not a species; the Cult of the Eaten Name (the game's Cultists) are mortals hollowed out by it; it opposes `deity.lunere`'s dreams.

- [ ] **Step 3: Curate, verify, commit**

```bash
ls docs/lore/30-outer-gods/*.md | wc -l   # Expected: 2
git add docs/lore/30-outer-gods && git commit -m "lore: outer-god entries (Quolzeth, Ulgrethu)"
```

---

### Task 7: The 11 planes

**Files:**
- Create: `docs/lore/40-planes/gleam.md` (worked, below)
- Create via lore-writer: `velor.md`, `gloam.md`, `cinderdeep.md`, `everbrine.md`, `skyvault.md`, `adamant.md`, `meridian.md`, `ironcourts.md`, `churn.md`, `unsounded.md`

- [ ] **Step 1: Write the worked entry**

`docs/lore/40-planes/gleam.md`:

```markdown
---
id: plane.gleam
status: canon
---
# The Gleam

**Tier:** Mirror plane · **Nature:** Bright mirror-world; fey-analogue, beauty with teeth

## Harmonic
When the Worldsong fixed Velor, its brightest overtones pooled into a mirror of the world
where everything is more vivid and nothing is safe. The Gleam keeps every promise
literally and every guest briefly.

## Inhabitants & Hazards
The Gnarlings are a feral splinter-faith of Verdessa lost to the Gleam's influence —
green things that learned the mirror's manners and forgot their mother's.

## Playability
Mirror portals in `GiantTree` / `RockCircle` sites; mid-difficulty zone with
inverted-color palette biomes (per the world-difficulty-zones spec).

> *Voice: a returned (mostly) expedition's debrief, era.embers, reliability uneven.*
```

- [ ] **Step 2: Dispatch lore-writer for the remaining 10 planes**

Ten dispatches, Task 3 prompt adjusted: target `docs/lore/40-planes/<file>.md`, match `gleam.md`'s sections (Harmonic / Inhabitants & Hazards / Playability / Voice line). Canon facts per plane come from the plane table in `00-cosmology.md` (id, nature, playable-via — keep all three unchanged). Special constraints: `unsounded.md` obeys the style-guide rule that the Unsounded is never explained and never fully playable ("thin places" only); `meridian.md` is sealed, opening only during ORACLE arc events; `ironcourts.md` and `churn.md` must stay consistent with `docs/lore/20-fiends/` (seven tiered courts; ever-dissolving abyss).

- [ ] **Step 3: Curate, verify, commit**

```bash
ls docs/lore/40-planes/*.md | wc -l   # Expected: 11
git add docs/lore/40-planes && git commit -m "lore: 11 plane entries"
```

---

### Task 8: The 6 era chronicles

**Files:**
- Create: `docs/lore/50-history/longdark.md` (worked, below)
- Create via lore-writer: `unsounded.md`, `worldsong.md`, `accord.md`, `severance.md`, `embers.md`

- [ ] **Step 1: Write the worked entry**

`docs/lore/50-history/longdark.md`:

```markdown
---
id: era.longdark
status: canon
---
# The Sealing & the Long Dark

**Order:** 5 of 6 · **Preceded by:** era.severance · **Followed by:** era.embers

## Chronicle
To end the Severance War, the Chorus did the one thing the Severed could not survive: it
stopped singing to the world. The Veil was sealed from the inside — gods and Severed
alike shut behind it. Velor kept its laws (the Song holds) but lost its singers. Mortals
endured a sunless generation; crops failed by starlight, cities moved underground.

## Saint Velken
A miner carried the first ember-lantern through the Long Dark, relighting hearth after
hearth. Solenne raised him into the Song's margins (`ascended.velken`). Every lantern lit
since is his rite.

## Scars in the world
Underground vaults, lantern-shrines, and the deep roads between old cities date to this era.

> *Voice: the Lantern Hall's official chronicle, compiled era.embers, reliable but pious.*
```

- [ ] **Step 2: Dispatch lore-writer for the remaining 5 eras**

Five dispatches, Task 3 prompt adjusted: target `docs/lore/50-history/<file>.md` (ids `era.unsounded`, `era.worldsong`, `era.accord`, `era.severance`, `era.embers`), match `longdark.md`'s sections (Order header / Chronicle / one named-focus section / Scars in the world / Voice line). Constraints: `era.unsounded` (order 1) is under one page of deliberate non-information (style-guide rule); `era.accord` (order 3) names Haniwa, Myrmidon, Terracotta, and the dwarven delves as flourishing cultures; `era.severance` (order 4) is the calamity that left the map's ruins and calcified the fiends; `era.embers` (order 6, current) frames every ORACLE event arc as "a crack in the Veil".

- [ ] **Step 3: Curate, verify, commit**

```bash
ls docs/lore/50-history/*.md | wc -l   # Expected: 6
git add docs/lore/50-history && git commit -m "lore: 6 era chronicles"
```

---

### Task 9: Machine-readable canon — `assets/lore/index.ron` + `common/src/lore.rs`

**Files:**
- Create: `assets/lore/index.ron`, `common/src/lore.rs`
- Modify: `common/src/lib.rs` (add `pub mod lore;` alphabetically between `pub mod lod;` and `pub mod lottery;`, ~line 53)

- [ ] **Step 1: Write the failing load test (TDD)**

Create `common/src/lore.rs` with only the test module, register `pub mod lore;` in `common/src/lib.rs`, run. Pattern exemplar: `test_all_skillset_assets` (`common/src/skillset_builder.rs:158`) — `use crate::assets::{AssetExt, Ron};` + `Ron::<T>::load_expect`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lore_index_loads_and_validates() {
        let index = LoreIndex::load();
        index.validate().unwrap_or_else(|errs| panic!("canon errors: {errs:#?}"));
        let tier = |t| index.deities.values().filter(|d| d.tier == t).count();
        assert_eq!(tier(DeityTier::Prime), 12);
        assert_eq!(tier(DeityTier::Severed), 5);
        assert_eq!(tier(DeityTier::Ascended), 2);
        assert_eq!(index.fiends.len(), 4);
        assert_eq!(index.outer.len(), 2);
        assert_eq!(index.planes.len(), 11);
        assert_eq!(index.eras.len(), 6);
    }
}
```

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common lore_index -- --nocapture`
Expected: FAIL to compile ("cannot find `LoreIndex`").

- [ ] **Step 2: Implement the types and loader**

Top of `common/src/lore.rs`:

```rust
//! Machine-readable lore canon (Project MYTHOS). Prose canon lives in `docs/lore/`;
//! this index is the id source of truth for AURORA/ORACLE and the canon lint.
//! Spec: docs/superpowers/specs/2026-06-10-lore-cosmology-design.md

use crate::assets::{AssetExt, Ron};
use hashbrown::HashMap;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeityTier { Prime, Severed, Ascended }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FiendPhylum { Devil, Demon }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlaneTier { Material, Mirror, Elemental, Celestial, Fiendish, Outer }

/// Divine domains; AURORA religions and the magic spec's ability schools key off these.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Domain {
    Light, Sun, Healing, Law, Death, Memory, Fate, War, Courage, Oaths,
    Knowledge, Magic, Stars, Sea, Storms, Voyages, Nature, Growth, Beasts,
    Forge, Craft, Stone, Home, Harvest, Hospitality, Moon, Dreams, Prophecy,
    Trickery, Luck, Roads, Commerce, Contracts, Wealth, Time, Seasons, Calendar,
    // Corrupted domains of the Severed
    Slaughter, Lies, Pacts, Blight, Rot, Undeath, Tyranny, Ruin, Destruction,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Deity {
    pub name: String,
    pub epithet: String,
    pub tier: DeityTier,
    pub domains: Vec<Domain>,
    #[serde(default)] pub allies: Vec<String>,
    #[serde(default)] pub enemies: Vec<String>,
    pub i18n_key: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Fiend { pub name: String, pub title: String, pub phylum: FiendPhylum, pub portfolio: String, pub i18n_key: String }

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OuterGod { pub designation: String, pub aspect: String, pub i18n_key: String }

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Plane { pub name: String, pub tier: PlaneTier, pub nature: String, pub i18n_key: String }

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Era { pub name: String, pub order: u8, pub i18n_key: String }

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LoreIndex {
    pub version: u32,
    pub deities: HashMap<String, Deity>,
    pub fiends: HashMap<String, Fiend>,
    pub outer: HashMap<String, OuterGod>,
    pub planes: HashMap<String, Plane>,
    pub eras: HashMap<String, Era>,
}

impl LoreIndex {
    pub fn load() -> Self { Ron::<LoreIndex>::load_expect("lore.index").read().0.clone() }

    pub fn contains_id(&self, id: &str) -> bool {
        self.deities.contains_key(id) || self.fiends.contains_key(id)
            || self.outer.contains_key(id) || self.planes.contains_key(id)
            || self.eras.contains_key(id)
    }

    /// Canon integrity: id-prefix conventions, dangling ally/enemy refs, era order.
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errs = Vec::new();
        for (id, d) in &self.deities {
            let prefix_ok = match d.tier {
                DeityTier::Prime => id.starts_with("deity."),
                DeityTier::Severed => id.starts_with("deity_dark."),
                DeityTier::Ascended => id.starts_with("ascended."),
            };
            if !prefix_ok { errs.push(format!("{id}: tier/prefix mismatch")); }
            for r in d.allies.iter().chain(d.enemies.iter()) {
                if !self.contains_id(r) { errs.push(format!("{id}: dangling ref {r}")); }
            }
        }
        for (map, pre) in [(self.fiends.keys().collect::<Vec<_>>(), "fiend."),
                           (self.outer.keys().collect(), "outer."),
                           (self.planes.keys().collect(), "plane."),
                           (self.eras.keys().collect(), "era.")] {
            for id in map.iter().filter(|i| !i.starts_with(pre)) {
                errs.push(format!("{id}: expected prefix {pre}"));
            }
        }
        let mut orders: Vec<u8> = self.eras.values().map(|e| e.order).collect();
        orders.sort_unstable();
        orders.dedup();
        if orders.len() != self.eras.len() { errs.push("duplicate era order".into()); }
        if errs.is_empty() { Ok(()) } else { Err(errs) }
    }
}
```

- [ ] **Step 3: Write `assets/lore/index.ron` in full**

```ron
LoreIndex(
    version: 1,
    deities: {
        "deity.solenne": Deity(name: "Solenne", epithet: "The Dawnmother", tier: Prime, domains: [Light, Sun, Healing, Law], allies: ["deity.nereth"], enemies: ["deity_dark.drazkhul", "outer.ulgrethu"], i18n_key: "lore-deity-solenne"),
        "deity.nereth": Deity(name: "Nereth", epithet: "The Quiet Door", tier: Prime, domains: [Death, Memory, Fate], allies: ["deity.solenne"], enemies: ["deity_dark.drazkhul"], i18n_key: "lore-deity-nereth"),
        "deity.veshtur": Deity(name: "Veshtur", epithet: "The Banner Unbroken", tier: Prime, domains: [War, Courage, Oaths], enemies: ["deity_dark.vukarra"], i18n_key: "lore-deity-veshtur"),
        "deity.yssira": Deity(name: "Yssira", epithet: "The Veiled Archivist", tier: Prime, domains: [Knowledge, Magic, Stars], enemies: ["deity_dark.szorvenn", "outer.ulgrethu"], i18n_key: "lore-deity-yssira"),
        "deity.maravel": Deity(name: "Maravel", epithet: "The Tidecaller", tier: Prime, domains: [Sea, Storms, Voyages], enemies: ["outer.quolzeth"], i18n_key: "lore-deity-maravel"),
        "deity.verdessa": Deity(name: "Verdessa", epithet: "The Greenmother", tier: Prime, domains: [Nature, Growth, Beasts], enemies: ["deity_dark.ghorvul"], i18n_key: "lore-deity-verdessa"),
        "deity.toldram": Deity(name: "Toldram", epithet: "The First Smith", tier: Prime, domains: [Forge, Craft, Stone], enemies: ["deity_dark.kelzhara"], i18n_key: "lore-deity-toldram"),
        "deity.hestrel": Deity(name: "Hestrel", epithet: "The Hearthkeeper", tier: Prime, domains: [Home, Harvest, Hospitality], allies: ["deity.verdessa"], i18n_key: "lore-deity-hestrel"),
        "deity.lunere": Deity(name: "Lunere", epithet: "The Pale Dreamer", tier: Prime, domains: [Moon, Dreams, Prophecy], enemies: ["outer.ulgrethu"], i18n_key: "lore-deity-lunere"),
        "deity.pell": Deity(name: "Pell", epithet: "The Manyfaced", tier: Prime, domains: [Trickery, Luck, Roads], i18n_key: "lore-deity-pell"),
        "deity.gildmar": Deity(name: "Gildmar", epithet: "The Open Hand", tier: Prime, domains: [Commerce, Contracts, Wealth], enemies: ["fiend.malverant"], i18n_key: "lore-deity-gildmar"),
        "deity.velora": Deity(name: "Velora", epithet: "The Wheel of Years", tier: Prime, domains: [Time, Seasons, Calendar], i18n_key: "lore-deity-velora"),
        "deity_dark.vukarra": Deity(name: "Vukarra", epithet: "The Red Thirst", tier: Severed, domains: [Slaughter], enemies: ["deity.veshtur"], i18n_key: "lore-deity-vukarra"),
        "deity_dark.szorvenn": Deity(name: "Szorvenn", epithet: "The Hollow Whisper", tier: Severed, domains: [Lies, Pacts], enemies: ["deity.yssira", "deity.gildmar"], i18n_key: "lore-deity-szorvenn"),
        "deity_dark.ghorvul": Deity(name: "Ghorvul", epithet: "The Famine Below", tier: Severed, domains: [Blight, Rot], enemies: ["deity.verdessa"], i18n_key: "lore-deity-ghorvul"),
        "deity_dark.drazkhul": Deity(name: "Drazkhul", epithet: "The Sleepless Crown", tier: Severed, domains: [Undeath, Tyranny], enemies: ["deity.nereth", "deity.solenne"], i18n_key: "lore-deity-drazkhul"),
        "deity_dark.kelzhara": Deity(name: "Kelzhara", epithet: "The Chained Flame", tier: Severed, domains: [Ruin, Destruction], enemies: ["deity.toldram"], i18n_key: "lore-deity-kelzhara"),
        "ascended.velken": Deity(name: "Saint Velken", epithet: "The Lantern-Bearer", tier: Ascended, domains: [Light], allies: ["deity.solenne"], i18n_key: "lore-deity-velken"),
        "ascended.auressa": Deity(name: "Auressa", epithet: "The Skyborn", tier: Ascended, domains: [Storms, Voyages], allies: ["deity.maravel"], i18n_key: "lore-deity-auressa"),
    },
    fiends: {
        "fiend.malverant": Fiend(name: "Malverant", title: "Lord of the First Ledger", phylum: Devil, portfolio: "Debt, soul-contracts, usury", i18n_key: "lore-fiend-malverant"),
        "fiend.serqitel": Fiend(name: "Serqitel", title: "The Gilded Judge", phylum: Devil, portfolio: "Corrupt law, perjured oaths, tribunals", i18n_key: "lore-fiend-serqitel"),
        "fiend.uzghorath": Fiend(name: "Uzghorath", title: "The Maw Unending", phylum: Demon, portfolio: "Gluttony, hordes, devouring", i18n_key: "lore-fiend-uzghorath"),
        "fiend.vyshka": Fiend(name: "Vyshka", title: "Queen of the Howling Waste", phylum: Demon, portfolio: "Storms of ruin, frenzy, stampede", i18n_key: "lore-fiend-vyshka"),
    },
    outer: {
        "outer.quolzeth": OuterGod(designation: "Quolzeth, That Which Waits Beneath the Brine", aspect: "Drowned immensity; pressure, brine, patience", i18n_key: "lore-outer-quolzeth"),
        "outer.ulgrethu": OuterGod(designation: "Ulgrethu, the Thought That Eats", aspect: "Devourer of names and minds; anti-memory", i18n_key: "lore-outer-ulgrethu"),
    },
    planes: {
        "plane.velor": Plane(name: "Velor", tier: Material, nature: "The sung world; the current game world", i18n_key: "lore-plane-velor"),
        "plane.gleam": Plane(name: "The Gleam", tier: Mirror, nature: "Bright mirror-world; fey-analogue, beauty with teeth", i18n_key: "lore-plane-gleam"),
        "plane.gloam": Plane(name: "The Gloam", tier: Mirror, nature: "Shadow mirror; where memory pools", i18n_key: "lore-plane-gloam"),
        "plane.cinderdeep": Plane(name: "The Cinderdeep", tier: Elemental, nature: "Elemental fire", i18n_key: "lore-plane-cinderdeep"),
        "plane.everbrine": Plane(name: "The Everbrine", tier: Elemental, nature: "Elemental water", i18n_key: "lore-plane-everbrine"),
        "plane.skyvault": Plane(name: "The Skyvault", tier: Elemental, nature: "Elemental air", i18n_key: "lore-plane-skyvault"),
        "plane.adamant": Plane(name: "The Adamant Reach", tier: Elemental, nature: "Elemental earth", i18n_key: "lore-plane-adamant"),
        "plane.meridian": Plane(name: "The High Meridian", tier: Celestial, nature: "Celestial realm of the Prime Chorus; sealed", i18n_key: "lore-plane-meridian"),
        "plane.ironcourts": Plane(name: "The Iron Courts", tier: Fiendish, nature: "Lawful hells, seven tiered courts", i18n_key: "lore-plane-ironcourts"),
        "plane.churn": Plane(name: "The Churn", tier: Fiendish, nature: "Chaotic abyss-analogue, ever-dissolving", i18n_key: "lore-plane-churn"),
        "plane.unsounded": Plane(name: "The Unsounded", tier: Outer, nature: "The unsung outside; never fully playable", i18n_key: "lore-plane-unsounded"),
    },
    eras: {
        "era.unsounded": Era(name: "Before the Song", order: 1, i18n_key: "lore-era-unsounded"),
        "era.worldsong": Era(name: "The Worldsong", order: 2, i18n_key: "lore-era-worldsong"),
        "era.accord": Era(name: "The Bright Accord", order: 3, i18n_key: "lore-era-accord"),
        "era.severance": Era(name: "The Severance War", order: 4, i18n_key: "lore-era-severance"),
        "era.longdark": Era(name: "The Sealing and the Long Dark", order: 5, i18n_key: "lore-era-longdark"),
        "era.embers": Era(name: "The Age of Embers", order: 6, i18n_key: "lore-era-embers"),
    },
)
```

Note: `i18n_key` values are **forward references** — `lore.ftl` ships in Phase 2 with the book reader; this plan does NOT wire them into the i18n completeness tests. Documented so nobody "fixes" the dangling keys prematurely.

- [ ] **Step 4: Run tests to verify they pass**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common lore -- --nocapture`
Expected: `lore_index_loads_and_validates ... ok`. If RON parsing fails, the error names the exact line — fix the asset, not the schema.

- [ ] **Step 5: Commit**

```bash
git add assets/lore common/src/lore.rs common/src/lib.rs
git commit -m "feat: machine-readable lore canon index with serde loader and validation"
```

---

### Task 10: Canon lint — frontmatter ids and denylist as Rust tests

**Files:**
- Modify: `common/src/lore.rs` (extend the `tests` module from Task 9)

Placement rationale: the lint lives beside the loader in `veloren-common` because it needs `LoreIndex` and the assets pipeline anyway; `docs/lore` is reached via `CARGO_MANIFEST_DIR/../docs/lore`, stable for this fork (`common` is never published standalone). The spec's `.claude/scripts/lore-lint.sh` is superseded — plain `cargo test` means CI gets it for free.

- [ ] **Step 1: Add the lint tests**

Append inside `mod tests` in `common/src/lore.rs`:

```rust
    use std::{fs, path::{Path, PathBuf}};

    fn lore_docs_dir() -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs/lore");
        assert!(dir.is_dir(), "docs/lore not found at {dir:?} — run from a full repo checkout");
        dir
    }

    fn walk_md(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).expect("read docs/lore dir") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() { walk_md(&path, out); }
            else if path.extension().is_some_and(|e| e == "md") { out.push(path); }
        }
    }

    fn frontmatter_id(text: &str) -> Option<String> {
        let mut lines = text.lines();
        if lines.next()?.trim() != "---" { return None; }
        for line in lines {
            let line = line.trim();
            if line == "---" { break; }
            if let Some(id) = line.strip_prefix("id:") { return Some(id.trim().to_string()); }
        }
        None
    }

    #[test]
    fn canon_doc_ids_resolve_in_index() {
        let index = LoreIndex::load();
        let mut files = Vec::new();
        walk_md(&lore_docs_dir(), &mut files);
        assert!(!files.is_empty(), "no markdown found under docs/lore");
        let mut errs = Vec::new();
        for file in &files {
            let name = file.file_name().unwrap().to_string_lossy().into_owned();
            // Inside a numbered canon dir (10-pantheon/ .. 50-history/)? Root files and
            // 60-npcs/ are exempt from *having* an id, never from resolving one they have.
            let in_numbered = file.parent().is_some_and(|p| {
                p.ancestors().any(|a| a.file_name().is_some_and(|n| {
                    let n = n.to_string_lossy();
                    ["10-", "20-", "30-", "40-", "50-"].iter().any(|pre| n.starts_with(pre))
                }))
            });
            let text = fs::read_to_string(file).expect("read lore file");
            match frontmatter_id(&text) {
                Some(id) if !index.contains_id(&id) =>
                    errs.push(format!("{name}: id `{id}` not in assets/lore/index.ron")),
                None if in_numbered && !name.starts_with('_') =>
                    errs.push(format!("{name}: missing `id:` frontmatter")),
                _ => {},
            }
        }
        assert!(errs.is_empty(), "canon lint failures:\n{}", errs.join("\n"));
    }

    #[test]
    fn no_forbidden_names_in_canon() {
        let dir = lore_docs_dir();
        let guide = fs::read_to_string(dir.join("70-style-guide.md")).expect("style guide");
        let mut denylist = Vec::new();
        let mut in_block = false;
        for line in guide.lines() {
            let t = line.trim();
            if t == "```denylist" { in_block = true; continue; }
            if in_block && t == "```" { break; }
            if in_block && !t.is_empty() { denylist.push(t.to_lowercase()); }
        }
        assert!(denylist.len() >= 20, "denylist block missing/truncated in 70-style-guide.md");
        let mut files = Vec::new();
        walk_md(&dir, &mut files);
        let mut errs = Vec::new();
        for file in files.iter().filter(|f| !f.ends_with("70-style-guide.md")) {
            let text = fs::read_to_string(file).expect("read lore file").to_lowercase();
            for token in text.split(|c: char| !c.is_alphanumeric() && c != '-') {
                if denylist.iter().any(|d| d == token) {
                    errs.push(format!("{}: forbidden name `{token}`", file.display()));
                }
            }
        }
        assert!(errs.is_empty(), "forbidden names found:\n{}", errs.join("\n"));
    }
```

- [ ] **Step 2: Run, then prove the lint catches both failure modes**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common lore -- --nocapture`
Expected: 3 tests PASS.

Negative check (don't skip): temporarily change `id: deity.solenne` to `id: deity.typo` in `solenne.md` and add the word `cthulhu` to `gleam.md`; rerun — expect BOTH tests to fail naming the file and offender. Revert, rerun, expect green.

- [ ] **Step 3: Commit**

```bash
git add common/src/lore.rs
git commit -m "test: canon lint — frontmatter id cross-check and forbidden-names denylist"
```

---

### Task 11: In-game surfacing wave 1 — loading tips + villager lore lines

**Files:**
- Modify: `assets/voxygen/i18n/en/main.ftl` (append to `loading-tips`, after `.a21` at ~line 121)
- Modify: `assets/voxygen/i18n/en/npc.ftl` (append to `npc-speech-villager_open`, after `.a7` at ~line 12)
- Modify: `docs/lore/70-style-guide.md` (Deferred section)

Both keys pick a random attribute at runtime (`get_variation_ctx`, `client/i18n/src/lib.rs:499`) — no code change. Non-EN languages never roll the new variants until translated; existing localization tests treat missing non-EN attributes as warnings.

- [ ] **Step 1: Add 5 lore loading tips**

Append to the `loading-tips =` block in `main.ftl` (`.b` series marks lore tips, per the spec):

```ftl
    .b0 = Sailors say the brine remembers. Sailors are right.
    .b1 = Every lit lantern is a small dawn — or so the faithful of the Dawnmother say.
    .b2 = The ruins that dot the world are scars of the Severance War, when the gods last walked.
    .b3 = Folk say the vampire courts never sleep because their king refused his own ending.
    .b4 = Where the world's song runs thin, things from outside listen in. Carry a light.
```

- [ ] **Step 2: Add 3 villager lore lines**

Append to `npc-speech-villager_open` in `npc.ftl` (currently `.a0`–`.a7`):

```ftl
    .a8 = My grandmother said every lit lantern is a little dawn. Keeps the dark honest.
    .a9 = They say the old ruins on the hills burned in a war between gods, long before us.
    .a10 = Don't whistle near the deep caves. Some places the world forgot to finish singing.
```

- [ ] **Step 3: Verify the Fluent files still parse**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-client-i18n`
Expected: PASS (validation tests parse every .ftl; a syntax slip fails here).
Optional eyeball: `veloren-run` skill, watch the connecting screen cycle `.b*` tips.

- [ ] **Step 4: Record the gated follow-ups (no code now)**

Data-driven per-culture site naming is **deferred to Phase 2** — verified too deep for this plan: `world/src/site/namegen.rs` is 935 lines of hardcoded syllable vectors inside `NameGen::generate()` / `generate_biome()`, and converting them to `assets/lore/namegen/*.ron` touches every world-gen call site. Append to `docs/lore/70-style-guide.md`:

```markdown
## Deferred to Phase 2
- Per-culture `NameGen` syllable tables as data (`assets/lore/namegen/*.ron`), replacing
  the hardcoded lists in `world/src/site/namegen.rs` (`generate()`, `generate_biome()`);
  lands with the canon-flavored dungeon titles (spec §7.4).
- `lore.ftl` + the `i18n_key` forward references in `assets/lore/index.ron` (spec §7.1).
- Readable book items — `ItemKind::Book` gap (spec §7.2) — and the lore dialogue topic (§7.3).
```

- [ ] **Step 5: Commit**

```bash
git add assets/voxygen/i18n/en/main.ftl assets/voxygen/i18n/en/npc.ftl docs/lore/70-style-guide.md
git commit -m "feat: first in-game lore strings — loading tips and villager small-talk"
```

---

### Task 12: Lint, format, changelog, finish

- [ ] **Step 1: CI-identical lint**

```bash
cargo clippy --all-targets --locked \
  --features="bin_cmd_doc_gen,bin_compression,bin_csv,bin_graphviz,bin_bot,bin_asset_migrate,asset_tweak,bin,stat,cli" \
  -- -D warnings
```
Expected: clean. `lore.rs` is the only new Rust surface; fix warnings there (no `#[allow]` without a justifying comment).

- [ ] **Step 2: Format**

Run: `cargo fmt --all -- --check` — if it fails, run `cargo fmt --all` and re-check. (RON and markdown are untouched by rustfmt.)

- [ ] **Step 3: Full test pass**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common -p veloren-client-i18n`
Expected: PASS, including the 3 lore tests.

- [ ] **Step 4: Changelog**

Add under `### Added` in `## [Unreleased]` of `CHANGELOG.md` (~line 15):

```markdown
- Original lore canon (Project MYTHOS): Lore Bible under `docs/lore/`, machine-readable
  `assets/lore/index.ron` with canon-lint tests, first lore loading tips and villager lines.
```

```bash
git add CHANGELOG.md && git commit -m "docs: changelog entry for lore canon Phase 1"
```

- [ ] **Step 5: Finish the branch**

Invoke `superpowers:finishing-a-development-branch` (and `veloren-review` before merging into `development`). Phase 2 follow-ups (books, dialogue topic, namegen data, `lore.ftl`) are recorded in `docs/lore/70-style-guide.md` and the spec's production plan.
