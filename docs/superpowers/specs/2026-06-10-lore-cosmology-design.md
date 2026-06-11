# Lore & Cosmology: Project MYTHOS — An Original Canon for the Fork

**Date:** 2026-06-10
**Scope:** Canon "Lore Bible" + machine-readable lore index + in-game delivery vectors
**Companion specs:** `2026-06-10-project-aurora-design.md` (NPC religions consume the pantheon), `2026-06-10-project-oracle-design.md` (world events reference lore arcs), `2026-06-10-world-difficulty-zones-design.md` (planes become playable zones), `2026-06-10-magic-abilities-design.md` (divine domains map to ability schools)

## Context

Our fork is growing systems that need a shared fiction: AURORA gives rtsim NPCs religions and cults, ORACLE generates world events with narrative framing, the difficulty-zones spec introduces portal-gated zones, and the magic spec needs divine domains to hang ability schools on. None of these can be coherent without a single source of truth for *who the gods are, what the planes are, and what happened in history*.

Upstream Veloren ships almost no explicit lore. What exists is implicit: a `Cultist` dungeon and profession, a sea boss named Dagon, a Mindflayer boss, villager bark lines about cultists, and procedurally generated site names. There is no creation myth, no pantheon, no named cosmology, and no document any system can consume.

This spec defines an **original** cosmology in the spirit of Matt Mercer's Exandria (rich pantheon, calamity-style mythic war, planar map) fused with Lovecraft-style cosmic horror (an outer tier of unknowable entities). **IP rule, non-negotiable:** every name, deity, plane, and era in our canon is an original creation. No Critical Role / WotC names or near-copies; no reuse of Lovecraft's named entities (no Cthulhu, no Dagon-as-our-god — the existing Dagon *boss* is retconned as a creature, see §4.5). The style is borrowed; the canon is ours.

## Goals / Non-Goals

| Goals | Non-Goals |
|---|---|
| A canon Lore Bible under `docs/lore/` that humans author and curate | Shipping all lore content in-game at once |
| A machine-readable lore index (`assets/lore/index.ron`) that AURORA, ORACLE, and content tooling consume programmatically | Replacing upstream gameplay systems |
| Retcon existing Veloren content (Cultists, Dagon, Mindflayer, dungeons, Sea Chapel) *into* the canon, not discard it | Renaming existing upstream assets/entities (breaks upstream merges) |
| A naming style guide with per-culture phonology so generated and authored names feel consistent | Voice acting, cutscenes, or quest-line scripting (later specs) |
| Lint/CI checks so referenced canon ids always resolve | A modding/plugin API for third-party lore |

## Current State: Verified Existing Lore in the Codebase

Everything below was verified in the working tree on 2026-06-10.

| Artifact | Location | Notes |
|---|---|---|
| i18n format is **Fluent (.ftl)**, not RON | `assets/voxygen/i18n/en/*.ftl` + `assets/voxygen/i18n/en/_manifest.ron` | Migration to Fluent already complete upstream. ~30 languages present. |
| Loading-screen tips exist | `assets/voxygen/i18n/en/main.ftl:95` — `loading-tips =` with attributes `.a0`–`.a21` | All current tips are gameplay hints ("Press X to..."); zero lore tips. Verified delivery vector. |
| Cultist faction flavor | `assets/voxygen/i18n/en/npc.ftl:147-160` — `npc-speech-villager_cultist_alarm` ("Death to the cultists!", etc.) | Villagers treat cultists as a known evil. No explanation of *what* they worship. |
| Cultist profession | `common/src/rtsim.rs:485` — `Profession::Cultist` (serde rename `"9"`) | rtsim NPCs can be cultists today. |
| Cultist dungeon | `world/src/site/mod.rs:89-114` — `SiteKind::Cultist`; generator in `world/src/site/plot/cultist.rs` | Final boss is the Mindflayer (`name-body-biped_large-mindflayer`, `assets/voxygen/i18n/en/name.ftl:608`). |
| Dagon | `assets/voxygen/i18n/en/name.ftl` (`name-body-quadruped_low-dagon`), `item/items/crafting.ftl` ("Source of Dagons Power"), `SiteKind::ChapelSite` (Sea Chapel dungeon) | A sea boss + "Dagonite" arthropods. Name collides with Lovecraft/Mesopotamian Dagon — we retcon it as a *creature species name*, never a god (§4.5). |
| Other mythic-flavored sites | `world/src/site/mod.rs` — `Gnarling`, `Adlet`, `Haniwa`, `Myrmidon`, `Terracotta`, `VampireCastle`, `Sahagin`, `DwarvenMine` | Cultures/dungeons with zero written backstory. Prime retcon targets. |
| rtsim factions | `rtsim/src/data/faction.rs` — `Faction { seed, leader, good_or_evil: bool, sentiments }` | The field carries a literal `// TODO: Very stupid, get rid of this`. AURORA replaces it with deity affiliation (§7.5). |
| Dialogue system | `rtsim/src/rule/npc_ai/dialogue.rs` + `assets/voxygen/i18n/en/dialogue.ftl` | Topic-based Q&A (`dialogue-question-site`, `-self`, `-sentiment`, directions, quests, rock-paper-scissors). No lore topic exists yet. |
| Quest dialogue strings | `assets/voxygen/i18n/en/quest/courier_quests.ftl` | Precedent for feature-scoped .ftl files — we mirror this with `lore.ftl`. |
| Readable items | `common/src/comp/inventory/item/mod.rs:352-383` — `ItemKind` enum | **Verified gap:** no `Book`/readable variant. Closest precedents: `ItemKind::RecipeGroup`, `ConsumableKind::Recipe`, `ItemKind::Quest`. §7.2 closes this. |
| Site name generation | `world/src/site/namegen.rs` — `NameGen` with hardcoded syllable lists (`generate()`, `generate_biome()`) | One global phonology for all cultures. §6.3 extends it. |
| i18n completeness tests | `client/i18n/src/lib.rs:659,667` — `validate_all_localizations`, `test_strict_all_localizations` | Existing CI hook we extend for lore strings. |
| "Mirrim" | — | **Does not exist anywhere** in the repo (grepped `*.rs`, `*.ron`, `*.ftl`, `*.md`). The setting has no canonical world name today — naming it is ours to do. |

**Conclusion:** Veloren gives us evocative *nouns* (Cultists, Mindflayer, Dagon, Sea Chapel, vampire castles, ruined civilizations) with no connective tissue. The canon below is written so every one of those nouns gets an explanation rather than a replacement.

## Cosmology Framework

This section defines the *structure* of the canon plus 1–2 worked examples per category. The full bible (every deity write-up, every era chronicle) is Phase 1 content work, authored into `docs/lore/`.

### 4.1 Creation Myth Skeleton: The Worldsong

- Before everything: **the Unsounded** — not darkness, not void, but *that which the Song never named*. (This is the far-realm tier; see §4.6.)
- Creation: the **Worldsong** — a chorus of primal voices (the gods-to-be) sang reality into structure. Each verse fixed a law of the world: stone, tide, flame, breath, growth, ending.
- The world itself is named **Velor** in High Eleth, "the sung place" — retconning the game's own name into canon. The mortal calendar goddess **Velora, the Wheel of Years** (§4.2) is its namesake and keeper.
- Wherever the Song is thin — deep caves, drowned trenches, the spaces behind mirrors — the Unsounded leaks back in. This single sentence is the lore engine for every horror hook in the game.

### 4.2 Pantheon Structure

Three tiers of divinity, top-down:

**Tier 1 — The Prime Chorus (12 deities).** The original singers. Post-Sealing (§5), they act only through domains, omens, and clergy.

| Id | Name | Epithet | Domains | Disposition | Existing-content hook |
|---|---|---|---|---|---|
| `deity.solenne` | Solenne | The Dawnmother | Light, sun, healing, law | Lawful good | Anti-undead faith; opposes `VampireCastle` |
| `deity.nereth` | Nereth | The Quiet Door | Death (natural), memory, fate | True neutral | Graveyards; abhors undeath |
| `deity.veshtur` | Veshtur | The Banner Unbroken | War (honorable), courage, oaths | Lawful neutral | Guard profession patron |
| `deity.yssira` | Yssira | The Veiled Archivist | Knowledge, magic, stars | Neutral | Patron of alchemists; Mindflayer is her domain *corrupted* (§4.6) |
| `deity.maravel` | Maravel | The Tidecaller | Sea, storms, voyages | Chaotic neutral | Sailors, `CoastalTown`, pirates pray insincerely |
| `deity.verdessa` | Verdessa | The Greenmother | Nature, growth, beasts | Neutral good | `GiantTree`; Gnarlings as feral splinter-cult |
| `deity.toldram` | Toldram | The First Smith | Forge, craft, stone | Lawful good | `DwarvenMine` ruins; smith profession |
| `deity.hestrel` | Hestrel | The Hearthkeeper | Home, harvest, hospitality | Neutral good | Campfire healing flavor; tavern shrines |
| `deity.lunere` | Lunere | The Pale Dreamer | Moon, dreams, prophecy | Chaotic good | Night events; opposed by `outer.ulgrethu` |
| `deity.pell` | Pell | The Manyfaced | Trickery, luck, roads | Chaotic neutral | Thieves and merchants both claim him |
| `deity.gildmar` | Gildmar | The Open Hand | Commerce, contracts, wealth | Lawful neutral | Merchant profession; coin flavor text |
| `deity.velora` | Velora | The Wheel of Years | Time, seasons, the calendar | True neutral | World name "Velor(en)"; in-game calendar events |

**Worked example 1 — Solenne, the Dawnmother.** First voice of the Worldsong; sang the verse of light. Iconography: an unlidded lantern. Clergy heal and hunt undeath. Retcon hook: the game's ubiquitous lanterns are folk-religion — every lit lantern is "a small dawn", a prayer to Solenne. Her ascended saint is Velken (Tier 3).

**Worked example 2 — Nereth, the Quiet Door.** Sang the final verse — that all songs end. Not evil; the gentlest of the Chorus. Undeath is the theft of her verse, which is why Solenne (light) and Nereth (death) are *allies*, inverting the cliché. AURORA religions of Nereth run funerals and oppose `deity_dark.drazkhul` cults.

**Tier 2 — The Severed (5 dark gods, betrayer-analogue).** Chorus members who, during the Severance War (§5), tried to rewrite the Song in their own voice and were cast out of it.

| Id | Name | Epithet | Corrupted domain (counterpart) |
|---|---|---|---|
| `deity_dark.vukarra` | Vukarra | The Red Thirst | Slaughter (Veshtur's war, stripped of honor) |
| `deity_dark.szorvenn` | Szorvenn | The Hollow Whisper | Lies, forbidden pacts (Yssira's knowledge, hoarded) |
| `deity_dark.ghorvul` | Ghorvul | The Famine Below | Blight, rot (Verdessa's growth, inverted) |
| `deity_dark.drazkhul` | Drazkhul | The Sleepless Crown | Undeath, tyranny (Nereth's door, forced open) |
| `deity_dark.kelzhara` | Kelzhara | The Chained Flame | Ruinous fire, destruction (Toldram's forge, unbound) |

**Worked example — Drazkhul, the Sleepless Crown.** A king among the singers who refused his own ending and tore Nereth's verse to keep his court alive forever. Retcon hook: `SiteKind::VampireCastle` becomes a court of Drazkhul; the Harvester-style undead bosses are his "crowned" servants. AURORA can spawn Drazkhul cults in regions with vampire castles.

**Worked example — Szorvenn, the Hollow Whisper.** Stole verses and sang them backwards into mortal ears. Patron of treacherous pacts; pirates who break Gildmar's contracts are said to have "whispered to the Hollow". Distinct from the Outer Gods: Szorvenn *wants* worship; the Outer tier does not even notice it.

**Tier 3 — The Ascended (demigods, open roster).** Mortals raised into the Song's margins. Structure: each Ascended is tied to one Prime and one in-game mechanic.

- **Saint Velken, the Lantern-Bearer** (`ascended.velken`, under Solenne): a miner who carried the first ember-lantern through the Long Dark (§5); patron of everyone who presses the lantern key. Loading-tip and item-flavor goldmine.
- **Auressa the Skyborn** (`ascended.auressa`, under Maravel): first mortal to ride the storm-winds; gliders and airships are "Auressa's wings". `GliderCourse` sites become her shrine-trials.

### 4.3 Fiendish Hierarchy

Fiends are not gods; they are *war-debris* — entities calcified out of the Severance War's spilled power. Two opposed phyla:

**Arch-Devils (lawful) — the Iron Courts (§4.4).** Bound by the letter of the Song; they cannot lie outright, only contract.

| Id | Name | Title | Portfolio |
|---|---|---|---|
| `fiend.malverant` | Malverant | Lord of the First Ledger | Debt, soul-contracts, usury |
| `fiend.serqitel` | Serqitel | The Gilded Judge | Corrupt law, perjured oaths, tribunals |

**Arch-Demons (chaotic) — the Churn (§4.4).** Unbound appetite; cannot contract, only consume.

| Id | Name | Title | Portfolio |
|---|---|---|---|
| `fiend.uzghorath` | Uzghorath | The Maw Unending | Gluttony, hordes, devouring |
| `fiend.vyshka` | Vyshka | Queen of the Howling Waste | Storms of ruin, frenzy, stampede |

Design rule: devils generate *quest and dialogue* content (deals, escape clauses — Gildmar clergy as the counter-faction); demons generate *combat event* content (ORACLE horde incursions). Hierarchies below the arch tier are Phase 3 content.

### 4.4 The Planes Map

Each plane gets an id, a one-line nature, and a playability path via the portal/zone system in `2026-06-10-world-difficulty-zones-design.md`.

| Id | Plane | Nature | Playable via |
|---|---|---|---|
| `plane.velor` | Velor (Material) | The sung world; current game world | Baseline |
| `plane.gleam` | The Gleam | Bright mirror-world; fey-analogue, beauty with teeth | Mirror portals in `GiantTree` / `RockCircle` sites; mid-difficulty zone, inverted-color palette biomes |
| `plane.gloam` | The Gloam | Shadow mirror; where memory pools | Portals in graveyards/crypts; stealth-flavored zone, Nereth pilgrimage content |
| `plane.cinderdeep` | The Cinderdeep | Elemental fire | Volcano/lava-cave portals; high-difficulty zone, fire-resist gear check |
| `plane.everbrine` | The Everbrine | Elemental water | Deep-sea trench portals near `ChapelSite`; underwater zone tech |
| `plane.skyvault` | The Skyvault | Elemental air | Mountain-peak portals; floating-island zone, glider-mandatory traversal |
| `plane.adamant` | The Adamant Reach | Elemental earth | `DwarvenMine` depths; mega-cave zone |
| `plane.meridian` | The High Meridian | Celestial realm of the Prime Chorus | Endgame; sealed (§5) — opens only during ORACLE arc events |
| `plane.ironcourts` | The Iron Courts | Lawful hells, seven tiered courts | Devil-contract questlines culminate in a court raid zone |
| `plane.churn` | The Churn | Chaotic abyss-analogue, ever-dissolving | Demon-incursion events spawn temporary breach zones |
| `plane.unsounded` | The Unsounded | Far-realm-analogue; the unsung outside | Never fully playable; "thin places" (corrupted dungeon variants) leak its rules in (§4.6) |

### 4.5 Retcon Rules for Existing Content

| Existing content | Canon retcon |
|---|---|
| Cultists (`Profession::Cultist`, `SiteKind::Cultist`) | The **Cult of the Eaten Name** — mortals hollowed out by `outer.ulgrethu`; their dungeon is a thin place |
| Mindflayer boss | An avatar-fragment of Ulgrethu, not a species — "a thought it left behind" |
| Dagon boss + Dagonites + Sea Chapel | "Dagon" is the *sailors' species-name* for spawn of `outer.quolzeth`; the Sea Chapel is a drowned thin place. The name stays on the creature; the god gets an original name |
| Gnarlings | Feral splinter-faith of Verdessa, lost to the Gleam's influence |
| Haniwa / Terracotta / Myrmidon ruins | Mortal empires destroyed in the Severance War (per-region lore, Phase 3) |
| VampireCastle | Court of Drazkhul (§4.2) |

### 4.6 The Outer Tier: The Unsounded

Cosmic-horror layer. Hard rules that keep it Lovecraftian and original:

1. **Unknowable:** Outer entities have no domains, no alignment, no worship-contract. Cults *think* they worship them; they are wrong.
2. **Names are approximations:** canon ids carry the epithet "designation", e.g. *"Quolzeth (a sailor's stammer, not a name)"*. The style guide (§6.3) mandates this framing in all flavor text.
3. **Mechanical hook — Veil corruption:** a stacking debuff (`BuffKind::VeilCorruption`, new variant in `common/src/comp/buff.rs`) applied in thin places and by outer-tier enemies: screen-edge distortion (post-FX), whispering subtitles (existing subtitle system, `hud/subtitles.ftl`), and at max stacks temporary false HUD readings. Detailed tuning belongs to the magic-abilities spec; the *canon* contract is: corruption comes only from the Unsounded, never from gods or fiends.

| Id | Designation | Aspect | Existing-content hook |
|---|---|---|---|
| `outer.quolzeth` | Quolzeth, That Which Waits Beneath the Brine | Drowned immensity; pressure, brine, patience | Dagon retcon, Sea Chapel, Sahagin sorcerers as half-changed thralls |
| `outer.ulgrethu` | Ulgrethu, the Thought That Eats | Devourer of names and minds; anti-memory | Mindflayer avatar, Cult of the Eaten Name, opposes Lunere's dreams |

## Historical Ages (Canon Skeleton)

Five eras; each gets an id and one chronicle file in `docs/lore/50-history/`.

| Id | Era | Summary |
|---|---|---|
| `era.unsounded` | Before the Song | Only the Unsounded. No time, no places. Deliberately under-documented — the style guide forbids precise lore here. |
| `era.worldsong` | The Worldsong | The Chorus sings Velor into being; planes crystallize as harmonics of the Song. |
| `era.accord` | The Bright Accord | Gods walk the world; mortal cultures (Haniwa, Myrmidon, Terracotta, the dwarven delves) flourish under divine patronage. |
| `era.severance` | The Severance War | The five Severed try to rewrite the Song. Calamity-analogue: empires burn, fiends calcify from spilled power, the ruins that dot the map are this war's scars. |
| `era.longdark` | The Sealing & the Long Dark | To end the war, the Chorus seals itself (and the Severed) behind the **Veil** — gods can no longer walk Velor. Mortals endure a sunless generation; Saint Velken carries the first lantern. |
| `era.embers` | The Age of Embers (current) | The present day. The Veil is thinning: cult resurgence, monsters stirring, thin places opening. Every ORACLE event arc is canonically "a crack in the Veil". |

## Canon Management

### 6.1 Directory Structure: `docs/lore/`

```
docs/lore/
├── 00-cosmology.md          # Worldsong, the Veil, tier model (this spec's §4 expanded)
├── 10-pantheon/             # one file per deity: solenne.md, nereth.md, ...
│   └── _severed/            # dark gods: drazkhul.md, szorvenn.md, ...
├── 20-fiends/               # iron-courts.md, churn.md, one file per arch-fiend
├── 30-outer-gods/           # quolzeth.md, ulgrethu.md (+ style rules for writing them)
├── 40-planes/               # one file per plane id
├── 50-history/              # one chronicle per era id
├── 60-npcs/                 # named legendary mortals, saints, cult leaders
└── 70-style-guide.md        # phonology, tone, retcon rules, forbidden-names list
```

Markdown front-matter on every file carries the canon id (`id: deity.solenne`) so docs and the machine index can be cross-linted.

### 6.2 Machine-Readable Canon: `assets/lore/index.ron`

RON (matches every other data file in the repo), loaded through `common-assets` like any other asset, hot-reloadable in dev. Single index, versioned:

```ron
LoreIndex(
    version: 1,
    deities: {
        "deity.solenne": Deity(
            name: "Solenne",
            epithet: "The Dawnmother",
            tier: Prime,
            domains: [Light, Sun, Healing, Law],
            allies: ["deity.nereth"],
            enemies: ["deity_dark.drazkhul", "outer.ulgrethu"],
            i18n_key: "lore-deity-solenne",
        ),
        // ...
    },
    planes: { "plane.gleam": Plane(name: "The Gleam", tier: Mirror, ...) },
    eras:   { "era.severance": Era(order: 3, i18n_key: "lore-era-severance") },
    fiends: { ... },
    outer:  { ... },
)
```

Consumers: AURORA resolves `Faction.deity: Option<String>` ids against this index; ORACLE event templates declare `lore_refs: ["era.embers", "outer.quolzeth"]`; the canon lint (§9) validates both. Rust types live in a new module `common/src/lore.rs` (plain serde structs, no ECS coupling).

### 6.3 Naming Language Style Guide (`docs/lore/70-style-guide.md`)

One phonology per culture so names are recognizable at a glance:

| Register | Phonology rule | Examples |
|---|---|---|
| Prime deities (High Eleth) | Open syllables, long vowels, liquids (l, r, n); feminine `-a/-e`, masculine `-ur/-am` | Solenne, Maravel, Toldram |
| The Severed | One harsh cluster (kh, zk, gh, vk) + closed final syllable | Drazkhul, Vukarra |
| Fiends — devils | Latinate, legal-sounding, 3 syllables | Malverant, Serqitel |
| Fiends — demons | Guttural, doubled consonants | Uzghorath, Vyshka |
| Outer tier | Deliberately awkward clusters; always framed as approximations | Quolzeth, Ulgrethu |
| Settlements | Existing `NameGen` syllables (`world/src/site/namegen.rs`); per-culture syllable sets added as data, not hardcode | — |

The guide also carries the **forbidden-names list**: any name from Exandria, Forgotten Realms, Greyhawk, or the Lovecraft mythos is banned even as homage, enforced by the canon lint's denylist.

## In-Game Delivery Vectors

| # | Vector | Where | State |
|---|---|---|---|
| 7.1 | i18n lore strings | New `assets/voxygen/i18n/en/lore.ftl` (mirrors the `quest/courier_quests.ftl` precedent); keys `lore-deity-*`, `lore-era-*`, `lore-plane-*` referenced from `index.ron` | New file, existing pipeline |
| 7.2 | Readable book items | **Gap (verified):** `ItemKind` (`common/src/comp/inventory/item/mod.rs:352`) has no readable variant. Add `ItemKind::Book { pages: Vec<String> }` where pages are i18n keys; a simple HUD reader window (egui) renders them. Item defs under new `assets/common/items/lore/` | New ItemKind + HUD widget (M) |
| 7.3 | NPC dialogue hooks | Add a `dialogue-question-lore` topic family to `assets/voxygen/i18n/en/dialogue.ftl` and a lore branch in `rtsim/src/rule/npc_ai/dialogue.rs::general`; answers vary by `Profession` (Alchemist quotes Yssira, Guard quotes Veshtur, Cultist lies) | Extends existing topic system |
| 7.4 | Site/dungeon naming | Extend `NameGen` (`world/src/site/namegen.rs`) with per-culture syllable tables loaded from `assets/lore/namegen/*.ron`; named dungeons get canon-flavored titles ("Court of the Sleepless Crown" for vampire castles) | Refactor hardcoded lists to data |
| 7.5 | Religion hooks for AURORA | Replace `Faction::good_or_evil: bool` (`rtsim/src/data/faction.rs`, has upstream TODO to remove it) with `deity: Option<String>` canon id; AURORA spec owns behavior, this spec owns the id space | Cross-spec contract |
| 7.6 | Event flavor for ORACLE | ORACLE event templates carry `lore_refs`; broadcast text pulls deity/era epithets from `lore.ftl` ("The Veil thins above the Sea Chapel...") | Cross-spec contract |
| 7.7 | Loading-screen tips | Append lore attributes to `loading-tips` in `assets/voxygen/i18n/en/main.ftl` (verified at line 95, currently `.a0`–`.a21` gameplay tips); add `.b0+` series of in-world flavor ("Sailors say the brine remembers. Sailors are right.") | Trivial, existing system |

## Production Plan

Writing-heavy throughout: content is AI-assisted generation with **mandatory human curation** — every canon file gets a human editorial pass before its id enters `index.ron`. Dev-days below assume that workflow.

### Phase 1 — Lore Bible Core (pantheon + planes + history)

| Deliverables | Milestones |
|---|---|
| `docs/lore/` populated: 00-cosmology, all 12 Prime + 5 Severed + 2 Ascended write-ups, 4 arch-fiends, 2 outer gods, 11 plane files, 6 era chronicles, 70-style-guide | M1: structure + style guide merged. M2: pantheon complete. M3: `index.ron` + `common/src/lore.rs` loading in `cargo test` |
| `assets/lore/index.ron` + `common/src/lore.rs` serde types + asset-load test | |

- **Tasks:** scaffold directories; write style guide first (it gates everything); draft deity files (AI-assisted, curated); define RON schema; implement loader + unit test; canon lint v1 (id cross-check between front-matter and index).
- **Complexity:** **M** — ~7 dev-days (2 code, 5 writing/curation).
- **Risks:** scope creep into full novels (mitigation: 1-page cap per deity file in Phase 1); naming collisions with published IP discovered late (mitigation: denylist check + a web search per name before merge).

### Phase 2 — In-Game Surfacing (books, dialogue, naming, tips)

| Deliverables | Milestones |
|---|---|
| `ItemKind::Book` + HUD reader; 10 first books placed as dungeon loot; `lore.ftl` (EN only); `dialogue-question-lore` topic with per-profession answers; lore loading tips; data-driven `NameGen` cultures | M1: book item readable in-game. M2: lore dialogue topic live in rtsim. M3: named dungeons use canon names on the map |

- **Tasks:** ItemKind variant + serde migration safety (old saves must load — additive enum variant only); egui reader window; loot-table entries (`assets/common/loot_tables/`); dialogue branch in `npc_ai/dialogue.rs`; namegen data extraction; tips PR.
- **Complexity:** **L** — ~11 dev-days (7 code, 4 writing). The book reader and save-compat are the bulk.
- **Risks:** upstream merge friction on `item/mod.rs` and `dialogue.rs` (mitigation: additive-only changes, no reordering of existing variants); translation debt — `lore.ftl` ships EN-only and `test_strict_all_localizations` must treat missing lore keys as warnings, not errors, for other languages.

### Phase 3 — Deep Content (per-region lore, NPC legends)

| Deliverables | Milestones |
|---|---|
| Per-region lore packs (each ruin culture gets its Severance-era chronicle); `docs/lore/60-npcs/` saints and villains; cult questline seeds for AURORA; ORACLE arc scripts referencing `era.embers` events; 30+ additional books | M1: first region pack (Haniwa) complete end-to-end (doc → index → books → dialogue). M2: three packs. M3: ORACLE arc "The Thinning Veil" shipped |

- **Tasks:** region-pack template; AI-assisted chronicle drafts with curation; book placement per dungeon type; dialogue variants per region; Veil-corruption buff implementation handoff to magic spec.
- **Complexity:** **L, ongoing** — ~13 dev-days for the first three region packs, then steady-state ~2 days per pack.
- **Risks:** canon drift as multiple authors/agents write (mitigation: lint + the style guide is the contract); content outpacing AURORA/ORACLE consumption (mitigation: M1 of this phase is gated on AURORA Phase 1 landing).

## Testing & QA

| Check | Mechanism |
|---|---|
| Canon id integrity | New lint script `.claude/scripts/lore-lint.sh` (CI + pre-merge): every id referenced in `docs/lore/` front-matter, `lore.ftl` key suffixes, AURORA faction configs, and ORACLE `lore_refs` must exist in `assets/lore/index.ron`; fails on dangling or duplicate ids |
| Forbidden-name denylist | Same lint greps new lore files against the denylist in `70-style-guide.md` |
| Index loads | Unit test in `common/src/lore.rs` loading `assets/lore/index.ron` via `common-assets` (runs in `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common`) |
| i18n completeness | Extend `client/i18n/src/lib.rs` tests (`validate_all_localizations`, line 659): every `i18n_key` in the index must resolve in EN; non-EN missing keys are warnings |
| Book item integrity | Asset test: every `ItemKind::Book` page key resolves; every book item appears in at least one loot table |
| Save compatibility | Manual checklist item for Phase 2: load a pre-Book-variant character save in dev build |

## Open Questions

1. Should the canon world name be surfaced as "Velor" in EN strings, or kept doc-side only until upstream-merge implications are assessed? (Leaning doc-side until Phase 2.)
2. Does `ItemKind::Book` reuse `ItemKind::Quest` rendering paths or get a dedicated model? Needs a voxel-art pass either way.
3. Veil corruption ownership: this spec defines the canon contract, but the buff implementation could land via the magic-abilities spec or the difficulty-zones spec — decide when both are drafted.
4. Do we localize deity *names* (Fluent supports it) or treat them as untranslatable proper nouns? Proper-noun policy is simpler and recommended, but needs a translator-facing note in `assets/voxygen/i18n/README.md`.
5. How much canon do we expose to server operators (e.g., a `/lore` admin command dumping the index) for community servers?
