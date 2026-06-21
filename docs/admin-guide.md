# Xindeler Admin Mode Guide

Admin mode is the set of privileged chat commands that let you teleport, spawn
entities, edit the world, moderate players, and configure test characters. The
commands are all typed into the in-game chat (press Enter to open chat, then
type `/command ...`).

## Roles

There are two privilege roles, ordered from lowest to highest:

- **Moderator** (lower) — moderation tooling: ban/kick/whitelist, teleport,
  locations, `/sudo`, etc.
- **Admin** (higher) — everything a Moderator can do, **plus** world editing,
  spawning, item/character tooling, and server/debug commands.

An **Admin can run everything a Moderator can**; the reverse is not true.

This fork ships **69 Admin-gated** commands and **17 Moderator-gated** commands
(86 privileged commands total). Player-only commands (chat channels, `/kill`,
`/help`, etc.) are not covered here.

**Enforcement is server-side.** For every gated command the server checks the
caller's *real* role before executing. You cannot fake a role from the client —
editing the client, spoofing a packet, or anything similar will simply be
rejected with a "no permission" message.

---

## How to become an admin

There are four ways to get a role, depending on whether you are in singleplayer,
running your own dedicated server, or boosting someone temporarily in-game.

### 1. Singleplayer — automatic

In singleplayer the local player is **auto-granted Admin**. At startup the
embedded server inserts the singleplayer account into the admin list as
`Role::Admin` (`server/src/settings/mod.rs:396`). You do not have to do
anything: just press Enter to open chat and start typing commands.

> Note: the code carries a TODO indicating this auto-grant may become opt-in in
> the future, so don't rely on it being permanent across versions.

### 2. Dedicated server, persistent (recommended for a real server)

Use the `server-cli` `admin` subcommand from a shell:

```bash
veloren-server-cli admin add <username> <role>
veloren-server-cli admin remove <username>
```

- `<role>` is `admin` or `moderator` (case-insensitive).
- This edits `admins.ron` in the server's data directory. Entries are keyed by
  the account **UUID** (resolved from the username via the auth/login provider),
  so the grant **persists across server restarts**.

This is the right approach for a long-lived server where you want a stable set of
staff.

### 3. Dedicated server, interactive console

While the server is running you can type the same commands directly into the
server's stdin console:

```text
admin add <username> <role>
admin remove <username>
```

This has the same effect as the CLI subcommand above (it edits `admins.ron`,
persistent across restarts).

### 4. In-game, temporary

An existing Admin can grant a **session-only** role to an *online* player with:

```text
/adminify <player> [role]
```

- `role` is `admin` or `moderator`. **Omit `role` to remove** the player's
  current temporary role.
- Constraints (server-enforced):
  - You **cannot assign a role higher than your own permanent role**.
  - You **cannot reassign the role of anyone at your role or higher**.
- Temporary roles are **lost on disconnect or server restart**. For a grant that
  survives restarts, use `admin add` (method 2/3) instead.

### Note on `--no-auth` dev servers

On a development server started with `--no-auth`, the same
`admin add <username> <role>` works: the UUID is derived directly from the
username (no real auth lookup). This is how dev bots and the test-harness
accounts get their admin role.

### Which should I use?

| Situation                                  | Use this                          |
|--------------------------------------------|-----------------------------------|
| Playing singleplayer                       | Nothing — you're already Admin    |
| My own dedicated server (lasting staff)    | `admin add <username> <role>`     |
| Quick temporary boost to someone online    | `/adminify <player> [role]`       |

---

## Command reference

Each entry lists the role tag, the syntax, what it does (from the in-game
description strings), its flags, and two examples. `<required>` arguments are
mandatory; `[optional]` arguments may be omitted (a trailing optional player or
target usually defaults to yourself).

### Roles & moderation

#### /adminify  **[Admin]**

**Syntax:** `/adminify <player> [role]`

**What it does:** Temporarily gives a player a restricted admin role, or removes
their current temporary role if `role` is omitted.

**Flags:**
- `player` — required, a player's name. The target whose role changes.
- `role` — optional, one of `admin` | `moderator`. Omit to remove the role.

**Examples:**
- `/adminify Mara moderator` — grants Mara a temporary Moderator role.
- `/adminify Mara` — removes Mara's temporary role.

#### /alias  **[Moderator]**

**Syntax:** `/alias <name>`

**What it does:** Change your alias (display name).

**Flags:**
- `name` — required, a single word. The new alias.

**Examples:**
- `/alias Shadowblade` — sets your alias to "Shadowblade".
- `/sudo Mara alias Trickster` — sets Mara's alias to "Trickster".

### Players & accounts (ban / kick / mute / whitelist)

#### /ban  **[Moderator]**

**Syntax:** `/ban <player> [overwrite] [ban duration] [reason]`

**What it does:** Ban a player by username, for an optional duration. Pass `true`
for `overwrite` to alter an existing ban.

**Flags:**
- `player` — required, a player's name.
- `overwrite` — optional, true/false. Overwrite an existing ban for this user.
- `ban duration` — optional, a single word (e.g. a duration string). Omit for a
  permanent ban.
- `reason` — optional, free text to end of line. The ban reason.

**Examples:**
- `/ban Griefer` — permanently bans Griefer.
- `/ban Griefer true 7d Repeated griefing` — overwrites Griefer's ban with a
  7-day ban and a reason.

#### /ban_ip  **[Moderator]**

**Syntax:** `/ban_ip <player> [overwrite] [ban duration] [reason]`

**What it does:** Like `/ban`, but also bans the IP address associated with the
user. Pass `true` for `overwrite` to alter an existing ban.

**Flags:**
- `player` — required, a player's name.
- `overwrite` — optional, true/false. Overwrite an existing ban.
- `ban duration` — optional, a single word. Omit for a permanent ban.
- `reason` — optional, free text to end of line. The ban reason.

**Examples:**
- `/ban_ip Spammer` — permanently bans Spammer and their IP.
- `/ban_ip Spammer true 30d Ban evasion` — overwrites with a 30-day IP ban.

#### /ban_log  **[Moderator]**

**Syntax:** `/ban_log <player> [max entries]`

**What it does:** Shows the ban-history log for a player. *(No dedicated
description in command.ftl — the source reuses `command-ban-ip-desc`.)*

**Flags:**
- `player` — required, a player's name.
- `max entries` — optional, integer. Maximum number of log entries to show
  (default 10).

**Examples:**
- `/ban_log Griefer` — shows up to 10 ban-log entries for Griefer.
- `/ban_log Griefer 50` — shows up to 50 entries.

#### /unban  **[Moderator]**

**Syntax:** `/unban <player>`

**What it does:** Removes the ban for the given username. A linked IP ban (if
any) is removed as well.

**Flags:**
- `player` — required, a player's name.

**Examples:**
- `/unban Griefer` — lifts Griefer's ban (and linked IP ban).
- `/unban Spammer` — lifts Spammer's ban.

#### /unban_ip  **[Moderator]**

**Syntax:** `/unban_ip <player>`

**What it does:** Removes only the IP ban for the given username (the user's own
ban remains).

**Flags:**
- `player` — required, a player's name.

**Examples:**
- `/unban_ip Spammer` — removes the IP ban applied via the "Spammer" account.
- `/unban_ip Griefer` — removes only the IP ban for Griefer.

#### /kick  **[Moderator]**

**Syntax:** `/kick <player> [reason]`

**What it does:** Kicks a player by username.

**Flags:**
- `player` — required, a player's name.
- `reason` — optional, free text to end of line. The kick reason.

**Examples:**
- `/kick AFKplayer` — kicks AFKplayer.
- `/kick AFKplayer Idle too long` — kicks with a reason.

#### /whitelist  **[Moderator]**

**Syntax:** `/whitelist <add/remove> <player>`

**What it does:** Adds or removes a username from the whitelist.

**Flags:**
- `add/remove` — required, a single word: `add` or `remove`.
- `player` — required, a player's name.

**Examples:**
- `/whitelist add Trusted` — adds Trusted to the whitelist.
- `/whitelist remove Trusted` — removes Trusted from the whitelist.

#### /server_physics  **[Moderator]**

**Syntax:** `/server_physics <player> [enabled] [reason]`

**What it does:** Sets or unsets server-authoritative physics for an account
(anti-cheat tool).

**Flags:**
- `player` — required, a player's name.
- `enabled` — optional, true/false. Whether server-authoritative physics is on.
- `reason` — optional, free text to end of line. The reason for the change.

**Examples:**
- `/server_physics Suspect true Suspected speed hacks` — forces server physics on
  for Suspect.
- `/server_physics Suspect false` — turns server physics back off for Suspect.

#### /disconnect_all_players  **[Admin]**

**Syntax:** `/disconnect_all_players <confirm>`

**What it does:** Disconnects all players from the server. Requires the literal
word `confirm` as the argument to actually run.

**Flags:**
- `confirm` — required, a single word. Must be `confirm` to proceed.

**Examples:**
- `/disconnect_all_players confirm` — disconnects everyone (e.g. before a
  restart).
- `/disconnect_all_players` — does nothing but prompts you to re-run with
  `confirm`.

### Teleport & movement

#### /goto  **[Admin]**

**Syntax:** `/goto <x> <y> <z> [Dismount from ship]`

**What it does:** Teleport to a world position.

**Flags:**
- `x`, `y`, `z` — required, floats. Destination coordinates.
- `Dismount from ship` — optional, true/false. Whether to dismount from a ship
  when teleporting.

**Examples:**
- `/goto 15000 15000 300` — teleports to those coordinates.
- `/sudo Mara goto 15000 15000 300` — teleports Mara to those coordinates.

#### /goto_rand  **[Admin]**

**Syntax:** `/goto_rand [Dismount from ship]`

**What it does:** Teleport to a random position.

**Flags:**
- `Dismount from ship` — optional, true/false. Whether to dismount from a ship.

**Examples:**
- `/goto_rand` — teleports you somewhere random.
- `/sudo Mara goto_rand` — teleports Mara somewhere random.

#### /jump  **[Admin]**

**Syntax:** `/jump <x> <y> <z> [Dismount from ship]`

**What it does:** Offset your current position by the given amount.

**Flags:**
- `x`, `y`, `z` — required, floats. The offset to add to your position.
- `Dismount from ship` — optional, true/false. Whether to dismount from a ship.

**Examples:**
- `/jump 0 0 50` — moves you up 50 blocks.
- `/jump 100 0 0` — moves you 100 blocks along the X axis.

#### /tp  **[Moderator]**

**Syntax:** `/tp [target] [Dismount from ship]`

**What it does:** Teleport to another entity (defaults to acting on yourself /
your target).

**Flags:**
- `target` — optional, an entity selector. The entity to teleport to.
- `Dismount from ship` — optional, true/false. Whether to dismount from a ship.

**Examples:**
- `/tp Mara` — teleports you to Mara.
- `/sudo Newbie tp Mara` — teleports Newbie to Mara.

#### /site  **[Moderator]**

**Syntax:** `/site <site name> [Dismount from ship]`

**What it does:** Teleport to a named site (town/dungeon).

**Flags:**
- `site name` — required, a site name (may contain spaces).
- `Dismount from ship` — optional, true/false. Whether to dismount from a ship.

**Examples:**
- `/site Whitehaven` — teleports you to the site "Whitehaven".
- `/sudo Mara site Whitehaven` — teleports Mara to that site.

#### /spot  **[Admin]**

**Syntax:** `/spot <spot kind>`

**What it does:** Find and teleport to the closest spot of a given kind.

**Flags:**
- `spot kind` — required, one of the spot kinds (see tab-completion).

**Examples:**
- `/spot <see tab-completion>` — teleports to the nearest spot of the chosen
  kind.
- `/sudo Mara spot <kind>` — teleports Mara to the nearest such spot.

#### /respawn  **[Moderator]**

**Syntax:** `/respawn`

**What it does:** Teleport to your waypoint.

**Flags:** No arguments.

**Examples:**
- `/respawn` — teleports you to your waypoint.
- `/sudo Mara respawn` — teleports Mara to her waypoint.

#### /set_waypoint  **[Admin]**

**Syntax:** `/set_waypoint`

**What it does:** Sets your waypoint to your current location.

**Flags:** No arguments.

**Examples:**
- `/set_waypoint` — sets your waypoint here.
- `/sudo Mara set_waypoint` — sets Mara's waypoint to her current location.

#### /rtsim_tp  **[Admin]**

**Syntax:** `/rtsim_tp <npc index> [Dismount from ship]`

**What it does:** Teleport to an rtsim NPC by index.

**Flags:**
- `npc index` — required, integer. The rtsim NPC index (see `/rtsim_npc`).
- `Dismount from ship` — optional, true/false. Whether to dismount from a ship.

**Examples:**
- `/rtsim_tp 42` — teleports to rtsim NPC index 42.
- `/rtsim_tp 42 false` — teleports there without dismounting from your ship.

### Character & testing

#### /set_level  **[Admin]**

**Syntax:** `/set_level <level>`

**What it does:** Sets the target's character level (1–60) for testing — no
grinding required. It works by setting earned XP so that level-gated systems
(attunement cap, item minimum-levels) treat the character as that level. It does
**not** grant skill points.

**Flags:**
- `level` — required, integer (1–60). The level to set.

**Examples:**
- `/set_level 30` — sets your own character to level 30.
- `/sudo SomePlayer set_level 10` — sets another player to level 10.

#### /make_test_char  **[Admin]**

**Syntax:** `/make_test_char <level> [class] [kit]`

**What it does:** One-shot test-character setup: sets the level, optionally sets
the class, and optionally grants a kit — all in a single command.

**Flags:**
- `level` — required, integer. The character level to set.
- `class` — optional, one of `warrior` | `mage` | `cleric` | `rogue`.
- `kit` — optional, any kit name (see `server.manifests.kits`; e.g. `all`,
  `test-attunement`).

**Examples:**
- `/make_test_char 10 mage test-attunement` — a level-10 mage with the attunement
  test rings.
- `/make_test_char 60 warrior all` — a max-level warrior with the full kit.

#### /set_class  **[player command — listed for completeness]**

**Syntax:** `/set_class <class>`

**What it does:** One-time class pick for legacy characters. *(This command has
role `None`, so any player may run it — it is not Admin/Moderator gated.)*

**Flags:**
- `class` — required, one of `warrior` | `mage` | `cleric` | `rogue`.

**Examples:**
- `/set_class mage` — locks in the mage class for a legacy character.
- `/set_class rogue` — locks in the rogue class.

#### /kit  **[Admin]**

**Syntax:** `/kit <kit_name>`

**What it does:** Places a set of items (a kit) into your inventory.

**Flags:**
- `kit_name` — required, any kit name (see `server.manifests.kits`; `all` grants
  every item).

**Examples:**
- `/kit all` — gives you the full "all" kit.
- `/kit test-attunement` — gives you the attunement test kit.

#### /skill_point  **[Admin]**

**Syntax:** `/skill_point <skill tree> [amount]`

**What it does:** Give yourself skill points for a particular skill tree.

**Flags:**
- `skill tree` — required, a skill-tree name (see tab-completion).
- `amount` — optional, integer. How many points (default 1).

**Examples:**
- `/skill_point <tree> 10` — grants 10 points in the chosen tree.
- `/skill_point <tree>` — grants 1 point in the chosen tree.

#### /skill_preset  **[Admin]**

**Syntax:** `/skill_preset <preset_name>`

**What it does:** Gives your character a desired preset of skills.

**Flags:**
- `preset_name` — required, a preset name (see tab-completion; `clear` resets).

**Examples:**
- `/skill_preset <preset>` — applies the chosen skill preset.
- `/skill_preset clear` — clears your skills.

#### /give_item  **[Admin]**

**Syntax:** `/give_item <item> [num]`

**What it does:** Give yourself some items. Use Tab to auto-complete item paths.

**Flags:**
- `item` — required, an asset path under `common.items.` (see tab-completion).
- `num` — optional, integer. Quantity (default 1).

**Examples:**
- `/give_item common.items.weapons.sword.steel-0` — gives you one steel sword.
- `/give_item common.items.consumable.potion_minor 10` — gives you 10 minor
  potions.

#### /health  **[Admin]**

**Syntax:** `/health <hp>`

**What it does:** Set your current health.

**Flags:**
- `hp` — required, integer. The health value to set.

**Examples:**
- `/health 100` — sets your health to 100.
- `/sudo Mara health 1` — sets Mara's health to 1.

#### /poise  **[Admin]**

**Syntax:** `/poise <poise>`

**What it does:** Set your current poise.

**Flags:**
- `poise` — required, integer. The poise value to set.

**Examples:**
- `/poise 100` — sets your poise to 100.
- `/sudo Mara poise 0` — sets Mara's poise to 0.

#### /buff  **[Admin]**

**Syntax:** `/buff <buff> [strength] [duration] [buff data spec]`

**What it does:** Cast a buff on a player.

**Flags:**
- `buff` — required, a buff name (see tab-completion).
- `strength` — optional, float. The buff strength.
- `duration` — optional, float. The duration in seconds.
- `buff data spec` — optional, a single word. Extra data some buffs require.

**Examples:**
- `/buff regeneration 0.5 30` — applies regeneration, strength 0.5, for 30s.
- `/sudo Mara buff frozen 1 10` — applies frozen to Mara for 10s.

#### /aura  **[Admin]**

**Syntax:** `/aura <aura_radius> [aura_duration] [new_entity] [aura_target] <aura_kind> [aura spec]`

**What it does:** Create an aura.

**Flags:**
- `aura_radius` — required, float. The aura's radius.
- `aura_duration` — optional, float. Duration in seconds.
- `new_entity` — optional, true/false. Attach to a new entity instead of you.
- `aura_target` — optional, a group-target option (see tab-completion).
- `aura_kind` — required, an aura-kind option (see tab-completion).
- `aura spec` — optional, a single word. Extra aura specification.

**Examples:**
- `/aura 10 30 false InGroup Buff` — a 10-radius group buff aura for 30s.
- `/aura 5 60 true All EnterSite` — a new-entity aura, radius 5, for 60s.

#### /repair_equipment  **[Admin]**

**Syntax:** `/repair_equipment [repair inventory]`

**What it does:** Repairs all equipped items (and optionally inventory items).

**Flags:**
- `repair inventory` — optional, true/false. Also repair inventory items
  (default true).

**Examples:**
- `/repair_equipment` — repairs equipped (and inventory) items.
- `/repair_equipment false` — repairs only equipped items.

#### /reset_recipes  **[Admin]**

**Syntax:** `/reset_recipes`

**What it does:** Resets your recipe book.

**Flags:** No arguments.

**Examples:**
- `/reset_recipes` — resets your recipe book.
- `/sudo Mara reset_recipes` — resets Mara's recipe book.

#### /scale  **[Admin]**

**Syntax:** `/scale <factor> [reset_mass]`

**What it does:** Scale your character.

**Flags:**
- `factor` — required, float. The scale factor.
- `reset_mass` — optional, true/false. Whether to reset mass to match (default
  true).

**Examples:**
- `/scale 2.0` — doubles your character's size.
- `/scale 0.5 false` — halves your size without resetting mass.

#### /body  **[Admin]**

**Syntax:** `/body <body>`

**What it does:** Change your body to a different species.

**Flags:**
- `body` — required, an entity/body name (see tab-completion).

**Examples:**
- `/body <see tab-completion>` — changes your body to the chosen species.
- `/sudo Mara body <species>` — changes Mara's body.

#### /set_body_type  **[Admin]**

**Syntax:** `/set_body_type <body type> [permanent]`

**What it does:** Set your body type (Female or Male).

**Flags:**
- `body type` — required, one of `Female` | `Male`.
- `permanent` — optional, true/false. Persist the change for the character
  (default false; only works on an online player character).

**Examples:**
- `/set_body_type Female` — sets your body type to Female (session).
- `/set_body_type Male true` — permanently sets your body type to Male.

#### /into_npc  **[Admin]**

**Syntax:** `/into_npc <entity_config>`

**What it does:** Convert yourself into an NPC. Use with care.

**Flags:**
- `entity_config` — required, an asset path under `common.entity.` (see
  tab-completion).

**Examples:**
- `/into_npc common.entity.wild.aggressive.wolf` — turns you into a wolf NPC.
- `/into_npc common.entity.village.guard` — turns you into a guard NPC.

#### /battlemode_force  **[Admin]**

**Syntax:** `/battlemode_force <battle mode>`

**What it does:** Change your battle-mode flag with no checks (no town/cooldown
restrictions).

**Flags:**
- `battle mode` — required, one of `pvp` | `pve`.

**Examples:**
- `/battlemode_force pvp` — forces your battle mode to PvP.
- `/battlemode_force pve` — forces your battle mode to PvE.

### Items & inventory

#### /dropall  **[Moderator]**

**Syntax:** `/dropall`

**What it does:** Drops all your items on the ground.

**Flags:** No arguments.

**Examples:**
- `/dropall` — drops your entire inventory at your feet.
- `/sudo Mara dropall` — makes Mara drop everything.

### Entities & spawning

#### /spawn  **[Admin]**

**Syntax:** `/spawn <alignment> <entity> [amount] [ai] [scale] [tethered]`

**What it does:** Spawn a test entity.

**Flags:**
- `alignment` — required, an alignment name (see tab-completion).
- `entity` — required, an entity/body name (see tab-completion).
- `amount` — optional, integer. How many to spawn (default 1).
- `ai` — optional, true/false. Whether the entity has AI (default true).
- `scale` — optional, float. Size scale (default 1.0).
- `tethered` — optional, true/false. Tether spawned entities to you (default
  false).

**Examples:**
- `/spawn enemy wolf 3` — spawns 3 hostile wolves.
- `/spawn npc wolf 1 true 2.0 true` — spawns one large, tethered, AI wolf NPC.

#### /make_npc  **[Admin]**

**Syntax:** `/make_npc <entity_config> [num]`

**What it does:** Spawn an entity from a config near you. Use Tab to
auto-complete.

**Flags:**
- `entity_config` — required, an asset path under `common.entity.`.
- `num` — optional, integer. How many (default 1).

**Examples:**
- `/make_npc common.entity.village.merchant` — spawns a merchant nearby.
- `/make_npc common.entity.wild.aggressive.wolf 5` — spawns 5 wolves.

#### /dummy  **[Admin]**

**Syntax:** `/dummy`

**What it does:** Spawns a training dummy.

**Flags:** No arguments.

**Examples:**
- `/dummy` — spawns a training dummy at your location.
- `/sudo Mara dummy` — spawns a training dummy at Mara's location.

#### /object  **[Admin]**

**Syntax:** `/object <object>`

**What it does:** Spawn an object.

**Flags:**
- `object` — required, an object name (see tab-completion).

**Examples:**
- `/object <see tab-completion>` — spawns the chosen object.
- `/sudo Mara object <name>` — spawns it at Mara's location.

#### /light  **[Admin]**

**Syntax:** `/light [r] [g] [b] [x] [y] [z] [strength]`

**What it does:** Spawn an entity that emits light.

**Flags:**
- `r`, `g`, `b` — optional, floats. The light color components.
- `x`, `y`, `z` — optional, floats. Position offset.
- `strength` — optional, float. Light strength (default 5.0).

**Examples:**
- `/light 1 0 0` — spawns a red light.
- `/light 1 1 1 0 0 5 10` — spawns a bright white light 5 blocks above you.

#### /airship  **[Admin]**

**Syntax:** `/airship [kind] [destination_degrees_ccw_of_east]`

**What it does:** Spawns an airship.

**Flags:**
- `kind` — optional, an airship kind (see tab-completion).
- `destination_degrees_ccw_of_east` — optional, float. Heading in degrees
  counter-clockwise from east.

**Examples:**
- `/airship` — spawns a default airship.
- `/airship <kind> 180` — spawns the chosen airship heading west.

#### /ship  **[Admin]**

**Syntax:** `/ship [kind] [tethered] [destination_degrees_ccw_of_east]`

**What it does:** Spawns a ship.

**Flags:**
- `kind` — optional, a ship kind (see tab-completion).
- `tethered` — optional, true/false. Tether the ship to the target/its mount
  (default false).
- `destination_degrees_ccw_of_east` — optional, float. Heading.

**Examples:**
- `/ship` — spawns a default ship.
- `/ship <kind> true 90` — spawns the chosen ship, tethered, heading north.

#### /portal  **[Admin]**

**Syntax:** `/portal <x> <y> <z> [requires_no_aggro] [buildup_time]`

**What it does:** Spawns a portal.

**Flags:**
- `x`, `y`, `z` — required, floats. Portal destination coordinates.
- `requires_no_aggro` — optional, true/false. Whether the portal requires you to
  not be in combat (default true).
- `buildup_time` — optional, float. Activation buildup time (default 5).

**Examples:**
- `/portal 15000 15000 300` — creates a portal to those coordinates.
- `/portal 15000 15000 300 false 2` — same, usable in combat, 2s buildup.

#### /campfire  **[Admin]**

**Syntax:** `/campfire`

**What it does:** Spawns a campfire.

**Flags:** No arguments.

**Examples:**
- `/campfire` — spawns a campfire at your location.
- `/sudo Mara campfire` — spawns a campfire at Mara's location.

#### /kill_npcs  **[Admin]**

**Syntax:** `/kill_npcs [radius] [--also-pets]`

**What it does:** Kills the NPCs (optionally within a radius, optionally
including pets).

**Flags:**
- `radius` — optional, float. Affect only NPCs within this radius (default 100).
- `--also-pets` — optional flag. Also kill pets.

**Examples:**
- `/kill_npcs` — kills NPCs within the default radius.
- `/kill_npcs 50 --also-pets` — kills NPCs and pets within 50 blocks.

#### /spot — *see Teleport & movement* (`/spot` teleports rather than spawns).

#### /outcome  **[Admin]**

**Syntax:** `/outcome <outcome>`

**What it does:** Create an outcome (a one-off world event/effect).

**Flags:**
- `outcome` — required, an outcome variant (see tab-completion).

**Examples:**
- `/outcome <see tab-completion>` — triggers the chosen outcome.
- `/sudo Mara outcome <variant>` — triggers it as Mara.

#### /death_effect  **[Admin]**

**Syntax:** `/death_effect <death_effect> <entity_config>`

**What it does:** Adds an on-death effect to the target entity.

**Flags:**
- `death_effect` — required, currently only `transform`.
- `entity_config` — required, an asset path under `common.entity.` (what the
  target transforms into on death).

**Examples:**
- `/death_effect transform common.entity.wild.aggressive.wolf` — on death, the
  target turns into a wolf.
- `/death_effect transform common.entity.village.guard` — on death, turns into a
  guard.

#### /mount  **[Admin]**

**Syntax:** `/mount <target>`

**What it does:** Mount an entity.

**Flags:**
- `target` — required, an entity selector. The entity to mount.

**Examples:**
- `/mount Mara` — mounts Mara.
- `/sudo Newbie mount Mara` — makes Newbie mount Mara.

#### /dismount  **[Admin]**

**Syntax:** `/dismount <target>`

**What it does:** Dismount if you are riding, or dismount whatever is riding you.

**Flags:**
- `target` — required, an entity selector.

**Examples:**
- `/dismount Mara` — dismounts the specified entity.
- `/sudo Mara dismount Newbie` — makes Mara dismount Newbie.

#### /tether  **[Admin]**

**Syntax:** `/tether <target> [automatic length]`

**What it does:** Tether another entity to yourself.

**Flags:**
- `target` — required, an entity selector. The entity to tether.
- `automatic length` — optional, true/false. Auto-compute tether length
  (default true).

**Examples:**
- `/tether Mara` — tethers Mara to you with automatic length.
- `/tether Mara false` — tethers Mara without automatic length.

#### /destroy_tethers  **[Admin]**

**Syntax:** `/destroy_tethers`

**What it does:** Destroys all tethers connected to you.

**Flags:** No arguments.

**Examples:**
- `/destroy_tethers` — frees you from all tethers.
- `/sudo Mara destroy_tethers` — destroys Mara's tethers.

### World, time & weather

#### /time  **[Admin]**

**Syntax:** `/time [time]`

**What it does:** Set the time of day.

**Flags:**
- `time` — optional, a time name (see tab-completion). Omit to query the current
  time.

**Examples:**
- `/time night` — sets the time to night.
- `/time` — prints the current time.

#### /time_scale  **[Admin]**

**Syntax:** `/time_scale [time scale]`

**What it does:** Set the scaling of delta time (speed up / slow down the
simulation clock).

**Flags:**
- `time scale` — optional, float. The scale factor (default 1.0). Omit to query.

**Examples:**
- `/time_scale 2.0` — runs time twice as fast.
- `/time_scale` — prints the current time scale.

#### /weather_zone  **[Admin]**

**Syntax:** `/weather_zone <weather kind> [radius] [time]`

**What it does:** Create a weather zone.

**Flags:**
- `weather kind` — required, one of `clear` | `rain` | `wind` | `storm`.
- `radius` — optional, float. Zone radius (default 500).
- `time` — optional, float. Zone duration in seconds (default 300).

**Examples:**
- `/weather_zone storm` — creates a default storm zone.
- `/weather_zone rain 800 600` — a rain zone, radius 800, for 600s.

#### /lightning  **[Admin]**

**Syntax:** `/lightning`

**What it does:** Lightning strike at your current position.

**Flags:** No arguments.

**Examples:**
- `/lightning` — calls a lightning strike where you stand.
- `/sudo Mara lightning` — strikes lightning at Mara's position.

#### /explosion  **[Admin]**

**Syntax:** `/explosion <radius>`

**What it does:** Explodes the ground around you.

**Flags:**
- `radius` — required, float. The explosion radius.

**Examples:**
- `/explosion 5` — a small explosion around you.
- `/explosion 20` — a large explosion around you.

#### /safezone  **[Moderator]**

**Syntax:** `/safezone [range]`

**What it does:** Creates a safezone (no-combat area).

**Flags:**
- `range` — optional, float. The safezone radius (default 100).

**Examples:**
- `/safezone` — creates a default-size safezone.
- `/safezone 50` — creates a 50-radius safezone.

#### /location  **[player command]**

**Syntax:** `/location <name>`

**What it does:** Teleport to a saved location. *(Role `None` — any player may
use it; listed here because it pairs with the Moderator location commands.)*

**Flags:**
- `name` — required, a single word. The location name.

**Examples:**
- `/location spawn` — teleports to the "spawn" location.
- `/location arena` — teleports to the "arena" location.

#### /create_location  **[Moderator]**

**Syntax:** `/create_location <name>`

**What it does:** Create a location at your current position.

**Flags:**
- `name` — required, a single word. Lowercase ASCII and underscores only.

**Examples:**
- `/create_location arena` — saves your position as "arena".
- `/create_location boss_room` — saves it as "boss_room".

#### /delete_location  **[Moderator]**

**Syntax:** `/delete_location <name>`

**What it does:** Delete a saved location.

**Flags:**
- `name` — required, a single word. The location to delete.

**Examples:**
- `/delete_location arena` — deletes the "arena" location.
- `/delete_location boss_room` — deletes "boss_room".

#### /reload_chunks  **[Admin]**

**Syntax:** `/reload_chunks [chunk_radius]`

**What it does:** Reloads chunks loaded on the server.

**Flags:**
- `chunk_radius` — optional, integer. How many chunks out to reload (default 6).

**Examples:**
- `/reload_chunks` — reloads chunks within the default radius.
- `/reload_chunks 10` — reloads chunks within a 10-chunk radius.

#### /clear_persisted_terrain  **[Admin]**

**Syntax:** `/clear_persisted_terrain <chunk_radius>`

**What it does:** Clears nearby persisted terrain edits.

**Flags:**
- `chunk_radius` — required, integer. The radius (in chunks) to clear.

**Examples:**
- `/clear_persisted_terrain 6` — clears persisted terrain within 6 chunks.
- `/clear_persisted_terrain 1` — clears only the immediate area.

#### /remove_lights  **[Admin]**

**Syntax:** `/remove_lights [radius]`

**What it does:** Removes all lights spawned by players.

**Flags:**
- `radius` — optional, float. Affect only lights within this radius (default 20).

**Examples:**
- `/remove_lights` — removes player lights within the default radius.
- `/remove_lights 100` — removes player lights within 100 blocks.

### Building & editing

#### /make_block  **[Admin]**

**Syntax:** `/make_block <block> [r] [g] [b]`

**What it does:** Make a block at your location with a color.

**Flags:**
- `block` — required, a block kind (see tab-completion).
- `r`, `g`, `b` — optional, integers (0–255). Block color (default 255 each).

**Examples:**
- `/make_block <kind>` — places the chosen block in white.
- `/make_block <kind> 255 0 0` — places it in red.

#### /make_sprite  **[Admin]**

**Syntax:** `/make_sprite <sprite>`

**What it does:** Make a sprite at your location. To define sprite attributes,
use RON syntax for a `StructureSprite`.

**Flags:**
- `sprite` — required, a sprite kind (see tab-completion).

**Examples:**
- `/make_sprite <see tab-completion>` — places the chosen sprite.
- `/sudo Mara make_sprite <kind>` — places it at Mara's location.

#### /make_volume  **[Admin]**

**Syntax:** `/make_volume [size]`

**What it does:** Create a volume (experimental).

**Flags:**
- `size` — optional, integer (1–127). The volume size (default 15).

**Examples:**
- `/make_volume` — creates a default-size volume.
- `/make_volume 32` — creates a 32-size volume.

#### /wiring  **[Admin]**

**Syntax:** `/wiring`

**What it does:** Create a wiring element.

**Flags:** No arguments.

**Examples:**
- `/wiring` — creates a wiring element at your location.
- `/sudo Mara wiring` — creates one at Mara's location.

#### /area_add  **[Admin]**

**Syntax:** `/area_add <name> <kind> <xlo> <xhi> <ylo> <yhi> <zlo> <zhi>`

**What it does:** Adds a new build area.

**Flags:**
- `name` — required, a single word. The area name.
- `kind` — required, an area kind (see tab-completion).
- `xlo`, `xhi`, `ylo`, `yhi`, `zlo`, `zhi` — required, integers. The bounding-box
  corners.

**Examples:**
- `/area_add plaza build 0 50 0 50 0 30` — adds a "plaza" build area.
- `/area_add vault no_durability 100 110 100 110 0 10` — adds a small "vault"
  area.

#### /area_list  **[Admin]**

**Syntax:** `/area_list`

**What it does:** List all build areas.

**Flags:** No arguments.

**Examples:**
- `/area_list` — lists all defined build areas.
- `/sudo Mara area_list` — lists them as Mara.

#### /area_remove  **[Admin]**

**Syntax:** `/area_remove <name> <kind>`

**What it does:** Removes a specified build area.

**Flags:**
- `name` — required, a single word. The area name.
- `kind` — required, an area kind (see tab-completion).

**Examples:**
- `/area_remove plaza build` — removes the "plaza" build area.
- `/area_remove vault no_durability` — removes the "vault" area.

#### /permit_build  **[Admin]**

**Syntax:** `/permit_build <area_name>`

**What it does:** Grants a player a bounded box they can build in.

**Flags:**
- `area_name` — required, a single word. The build area to permit.

**Examples:**
- `/permit_build plaza` — grants build permission in "plaza".
- `/sudo Mara permit_build plaza` — grants it via Mara.

#### /revoke_build  **[Admin]**

**Syntax:** `/revoke_build <area_name>`

**What it does:** Revokes a player's build-area permission.

**Flags:**
- `area_name` — required, a single word. The build area.

**Examples:**
- `/revoke_build plaza` — revokes "plaza" build permission.
- `/sudo Mara revoke_build plaza` — revokes it via Mara.

#### /revoke_build_all  **[Admin]**

**Syntax:** `/revoke_build_all`

**What it does:** Revokes all build-area permissions for a player.

**Flags:** No arguments.

**Examples:**
- `/revoke_build_all` — revokes all of your build permissions.
- `/sudo Mara revoke_build_all` — revokes all of Mara's build permissions.

### Server & debug

#### /set_motd  **[Admin]**

**Syntax:** `/set_motd [locale] [message]`

**What it does:** Set the server's message of the day (description).

**Flags:**
- `locale` — optional, a single word. The locale to set it for.
- `message` — optional, free text to end of line. The MOTD text (omit to remove).

**Examples:**
- `/set_motd en Welcome to Xindeler!` — sets the English MOTD.
- `/set_motd en` — removes the English MOTD.

#### /disconnect_all_players — *see Players & accounts* above.

#### /version  **[player command]**

**Syntax:** `/version`

**What it does:** Prints the server version. *(Role `None` — any player may use
it.)*

**Flags:** No arguments.

**Examples:**
- `/version` — prints the server version.
- `/sudo Mara version` — prints it as Mara.

#### /debug_column  **[Admin]**

**Syntax:** `/debug_column <x> <y>`

**What it does:** Prints some debug information about a column.

**Flags:**
- `x`, `y` — required, integers. The column coordinates.

**Examples:**
- `/debug_column 15000 15000` — prints debug info for that column.
- `/debug_column 0 0` — prints debug info for column (0, 0).

#### /debug_ways  **[Admin]**

**Syntax:** `/debug_ways <x> <y>`

**What it does:** Prints some debug information about a column's ways.

**Flags:**
- `x`, `y` — required, integers. The column coordinates.

**Examples:**
- `/debug_ways 15000 15000` — prints ways debug info for that column.
- `/debug_ways 0 0` — prints ways debug info for column (0, 0).

#### /gizmos  **[Admin]**

**Syntax:** `/gizmos <kind> [target]`

**What it does:** Manage gizmo (debug-visualization) subscriptions.

**Flags:**
- `kind` — required, one of `All` | `None` | a specific gizmo subscription (see
  tab-completion).
- `target` — optional, an entity selector. The entity to visualize.

**Examples:**
- `/gizmos All` — subscribes to all gizmos for yourself.
- `/gizmos None Mara` — clears gizmo subscriptions for Mara.

#### /gizmos_range  **[Admin]**

**Syntax:** `/gizmos_range <range>`

**What it does:** Change the range of gizmo subscriptions.

**Flags:**
- `range` — required, float. The gizmo range (default 32).

**Examples:**
- `/gizmos_range 64` — sets gizmo range to 64.
- `/gizmos_range 16` — sets gizmo range to 16.

#### /lantern  **[Admin]**

**Syntax:** `/lantern <strength> [r] [g] [b]`

**What it does:** Change your lantern's strength and color.

**Flags:**
- `strength` — required, float. The flame strength.
- `r`, `g`, `b` — optional, floats. The flame color (default 1.0 each).

**Examples:**
- `/lantern 8` — brightens your lantern.
- `/lantern 5 1 0 0` — makes your lantern glow red.

#### /rtsim_info  **[Admin]**

**Syntax:** `/rtsim_info <npc index>`

**What it does:** Display information about an rtsim NPC.

**Flags:**
- `npc index` — required, integer. The rtsim NPC index.

**Examples:**
- `/rtsim_info 42` — prints info for rtsim NPC 42.
- `/rtsim_info 0` — prints info for rtsim NPC 0.

#### /rtsim_npc  **[Admin]**

**Syntax:** `/rtsim_npc <query> [max number]`

**What it does:** List rtsim NPCs matching a query (e.g. `simulated,merchant`)
in order of distance.

**Flags:**
- `query` — required, a single word. The filter query.
- `max number` — optional, integer. Max results (default 20).

**Examples:**
- `/rtsim_npc merchant` — lists the nearest merchant NPCs.
- `/rtsim_npc simulated,merchant 5` — lists the 5 nearest simulated merchants.

#### /rtsim_chunk  **[Admin]**

**Syntax:** `/rtsim_chunk`

**What it does:** Display information about the current chunk from rtsim.

**Flags:** No arguments.

**Examples:**
- `/rtsim_chunk` — prints rtsim info for your current chunk.
- `/sudo Mara rtsim_chunk` — prints it for Mara's chunk.

#### /rtsim_purge  **[Admin]**

**Syntax:** `/rtsim_purge <on next startup>`

**What it does:** Purge rtsim data on next startup. *(Requires a real, permanent
admin — a temporary admin cannot purge rtsim data.)*

**Flags:**
- `on next startup` — required, true/false. Whether to purge on next startup.

**Examples:**
- `/rtsim_purge true` — schedules an rtsim data purge for the next startup.
- `/rtsim_purge false` — cancels a scheduled purge.

#### /sudo  **[Moderator]**

**Syntax:** `/sudo <target> <command ...>`

**What it does:** Run a command as if you were another entity. *(You cannot sudo
players with a role higher than your own, and Moderators cannot sudo
non-players.)*

**Flags:**
- `target` — required, an entity selector. The entity to run the command as.
- `command ...` — required, a full command (with its own arguments) to run.

**Examples:**
- `/sudo Mara goto 15000 15000 300` — teleports Mara as if she ran `/goto`.
- `/sudo Newbie set_level 10` — runs `/set_level 10` on Newbie (Admin only,
  since `/set_level` is Admin-gated).

---

## Tips

- **In-game help & completion.** `/help` lists all commands; `/help <command>`
  shows that command's usage and description. Tab-completion fills in command
  names and argument values — especially useful for the large generated lists
  (items, entities, sprites, kits, sites, airships) that this guide points to
  with "see tab-completion".
- **Run as another player.** `/sudo <player> <command ...>` (Admin/Moderator)
  executes any command as another entity — the cleanest way to apply a
  self-targeting command (like `/set_level`, `/health`, `/respawn`) to someone
  else. Remember the underlying command's own role gate still applies.
- **Self-targeting defaults.** Many commands act on you when the optional
  player/target argument is omitted (e.g. `/tp`, `/health`, `/poise`,
  `/respawn`).
