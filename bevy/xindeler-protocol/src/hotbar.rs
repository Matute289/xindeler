//! BL-82 EM-5.3 — the skillbar/hotbar mirror + drag-to-assign request.
//!
//! Follows the exact `NetHealth`/`NetLoadout`/EM-5.2 `NetEnergy` pattern
//! (spec §3.2): a small read-only `Net*` projection of the sim's
//! `common::comp::ability::{ActiveAbilities, AbilityPool, AbilityCooldowns}`,
//! plus the client -> server request that lets the hotbar UI's real
//! drag-drop primitive (`xindeler_ui::slot`) actually rebind a slot.
//!
//! ## Why [`NetAbilities`] carries BOTH a machine value and a display string
//! [`NetHotbarSlot::aux`] is the round-trippable
//! [`NetAuxiliaryAbility`] (mirrors `common::comp::ability::AuxiliaryAbility`
//! 1:1, indices widened to `u32` for a stable wire type) — the client needs
//! this to send a REBIND back (a display string alone can't be turned back
//! into a `change_ability` call). [`NetHotbarSlot::ability_id`] is the
//! already-resolved display id (`common::comp::ability::Ability::ability_id`,
//! the same resolution the legacy HUD's `skillbar.rs` uses for icon/i18n
//! lookup) — `None` when the slot resolves to nothing display-worthy (e.g. an
//! `Innate` slot gated on a skill nobody has unlocked yet). "Project, don't
//! dump" (spec §3.2): this is the compact shape a hotbar icon needs, not the
//! sim's own `ActiveAbilities`/`AbilityPool` structs.
//!
//! ## The write half: real drag-to-assign, the listen-server posture
//! [`AssignHotbarSlot`] is the real replicon client message (wire shape for a
//! FUTURE genuinely-remote client, registered like `PlayerInput`/EM-5.4's
//! `ChatSendRequest`) — no server-side handler exists for it yet (same
//! "future-remote-ready, dormant today" posture EM-5.4/EM-5.8 already
//! established for their own wire messages). [`LocalAssignHotbarSlot`] is
//! the one that actually drives gameplay TODAY: a plain in-process Bevy
//! message the client's hotbar UI writes directly, drained by
//! `xindeler-sim-bridge::hotbar::apply_local_hotbar_assignment`, which calls
//! straight into the embedded player's real `client::Client::change_ability`
//! — a genuine client->server network send over the loopback socket, never
//! a direct ECS write (isolation-law rule 4) — exactly EM-5.8's
//! `LocalGroupAction`/`apply_local_group_actions` precedent
//! (`bevy/xindeler-sim-bridge/src/social.rs`).

use bevy::ecs::{component::Component, message::Message};
use serde::{Deserialize, Serialize};

/// Mirrors `common::comp::ability::AuxiliaryAbility` 1:1 (indices widened to
/// `u32` — a stable wire type independent of the sim's own `usize`).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum NetAuxiliaryAbility {
    MainWeapon(u32),
    OffWeapon(u32),
    Glider(u32),
    Innate(u32),
    #[default]
    Empty,
}

impl NetAuxiliaryAbility {
    /// Packs this value into a plain `u64` (BL-82 EM-5.7): the diary's
    /// Abilities tab (`xindeler_protocol::skillset::NetAbilityPool`) uses this
    /// encoding to make each pool entry a valid `xindeler_ui::slot::
    /// SlotAddress` for dragging onto the hotbar — `xindeler-client::hotbar`'s
    /// drop handler unpacks it back via [`Self::from_slot_address_raw`] to
    /// resolve which ability was dragged. Plain `u64` (not a dependency on
    /// `xindeler_ui`) since that's the exact shape `SlotAddress` itself
    /// wraps — this crate stays UI-toolkit-agnostic.
    #[must_use]
    pub fn to_slot_address_raw(self) -> u64 {
        let (tag, idx): (u64, u64) = match self {
            Self::MainWeapon(i) => (0, u64::from(i)),
            Self::OffWeapon(i) => (1, u64::from(i)),
            Self::Glider(i) => (2, u64::from(i)),
            Self::Innate(i) => (3, u64::from(i)),
            Self::Empty => (4, 0),
        };
        (tag << 32) | idx
    }

    /// The inverse of [`Self::to_slot_address_raw`]. An unrecognized tag
    /// (never produced by the packer, but a defensive default all the same)
    /// resolves to [`Self::Empty`] rather than panicking.
    #[must_use]
    pub fn from_slot_address_raw(raw: u64) -> Self {
        let tag = raw >> 32;
        // No `cast_possible_truncation` suppression needed: clippy's range
        // analysis already proves `raw & 0xFFFF_FFFF` fits in `u32` from the
        // mask alone.
        let idx = (raw & 0xFFFF_FFFF) as u32;
        match tag {
            0 => Self::MainWeapon(idx),
            1 => Self::OffWeapon(idx),
            2 => Self::Glider(idx),
            3 => Self::Innate(idx),
            _ => Self::Empty,
        }
    }
}

/// One hotbar ("auxiliary ability") slot's projected content — see this
/// module's own doc comment for why both fields exist.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct NetHotbarSlot {
    pub aux: NetAuxiliaryAbility,
    /// The resolved display/icon id (`Ability::ability_id`), or `None` if it
    /// resolves to nothing display-worthy yet (e.g. a skill-gated innate).
    pub ability_id: Option<String>,
}

/// Replicated resolved ability-pool/hotbar projection for an entity (BL-82
/// EM-5.3). `primary`/`secondary` are the M1/M2 weapon abilities — display
/// only; the sim's own `PrimaryAbility`/`SecondaryAbility` are NOT
/// user-rebindable (`Tool`/`Empty` only), so there is no matching slot
/// address for them. `slots` is the CURRENT weapon context's auxiliary
/// ability set (`ActiveAbilities::auxiliary_set`) — its length is exactly
/// however many slots the sim currently supports (`ActiveAbilities::limit`,
/// `Some(5)` by default at character creation, `server/src/
/// character_creator.rs`) — this mirror does NOT pad to a hardcoded 10; the
/// hotbar UI renders exactly this many real, currently-usable slots (each
/// labelled with its `GameInput::Slot{n}` keybind for `n <= 10`), so a
/// future skill/perk that raises the limit shows up automatically with no
/// UI-side change.
#[derive(Component, Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct NetAbilities {
    pub primary: Option<String>,
    pub secondary: Option<String>,
    pub slots: Vec<NetHotbarSlot>,
}

/// One ability id's remaining cooldown (BL-82 EM-5.3), flattened from the
/// sim's `AbilityCooldowns` (`ready_at.0 - now.0`). Only entries CURRENTLY
/// cooling down are carried — an absent `ability_id` means "ready" (spec
/// §3.2 "project, don't dump"; matches `AbilityCooldowns` itself, which
/// prunes expired entries on every `set`).
///
/// v1 cut (documented, not silently skipped): the sim's `AbilityCooldowns`
/// stores only the absolute ready-at time, not the ORIGINAL cooldown
/// duration — carrying a `total_secs` here would mean duplicating the
/// ability-resolution machinery `Ability::ability_id` already owns just to
/// look up `AbilityMeta::cooldown` for an arbitrary ability id string. The
/// hotbar UI instead infers a per-slot "sweep total" client-side (the
/// largest `remaining_secs` observed since the ability last read as ready,
/// `xindeler-client::hotbar::CooldownTotals`) — self-correcting the first
/// time a cooldown is seen, good enough for a visual sweep without a second
/// sim-side cooldown-duration channel.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct NetCooldownEntry {
    pub ability_id: String,
    pub remaining_secs: f32,
}

/// Replicated per-entity cooldown snapshot (BL-82 EM-5.3), one entry per
/// ability id currently cooling down.
#[derive(Component, Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct NetCooldowns(pub Vec<NetCooldownEntry>);

/// Client -> server: bind an ability into a hotbar auxiliary slot (BL-82
/// EM-5.3 drag-to-assign). The real replicon wire shape for a FUTURE
/// genuinely-remote client — see this module's own doc comment for why the
/// listen-server path does not read this today (`LocalAssignHotbarSlot`
/// does).
#[derive(Message, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct AssignHotbarSlot {
    pub slot: u32,
    pub ability: NetAuxiliaryAbility,
}

/// The in-process listen-server counterpart of [`AssignHotbarSlot`] — see
/// this module's own doc comment.
#[derive(Message, Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalAssignHotbarSlot {
    pub slot: u32,
    pub ability: NetAuxiliaryAbility,
}

#[cfg(test)]
mod tests {
    use bevy::{app::App, ecs::message::Messages, prelude::*};
    use bevy_replicon::{
        prelude::{FromClient, RepliconPlugins, ServerPlugin},
        test_app::ServerTestAppExt,
    };

    use super::*;
    use crate::XindelerProtocolPlugin;

    fn new_app() -> App {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::state::app::StatesPlugin,
            RepliconPlugins.set(ServerPlugin::new(PostUpdate)),
            XindelerProtocolPlugin,
        ))
        .finish();
        app
    }

    /// BL-82 EM-5.3 acceptance (the mirror half): `NetAbilities`/
    /// `NetCooldowns` round-trip server -> client exactly like every other
    /// `Net*` mirror (spec §3.2/plan "every mirror PR gets a round-trip
    /// test").
    #[test]
    fn hotbar_mirror_replicates() {
        use bevy_replicon::prelude::Replicated;

        let mut server_app = new_app();
        let mut client_app = new_app();
        server_app.connect_client(&mut client_app);

        let abilities = NetAbilities {
            primary: Some("common.abilities.sword.primary".to_owned()),
            secondary: None,
            slots: vec![
                NetHotbarSlot {
                    aux: NetAuxiliaryAbility::MainWeapon(0),
                    ability_id: Some("common.abilities.sword.m1".to_owned()),
                },
                NetHotbarSlot {
                    aux: NetAuxiliaryAbility::Empty,
                    ability_id: None,
                },
            ],
        };
        let cooldowns = NetCooldowns(vec![NetCooldownEntry {
            ability_id: "common.abilities.sword.m1".to_owned(),
            remaining_secs: 2.5,
        }]);

        server_app
            .world_mut()
            .spawn((Replicated, abilities.clone(), cooldowns.clone()));

        server_app.update();
        server_app.exchange_with_client(&mut client_app);
        client_app.update();

        let mut q = client_app
            .world_mut()
            .query::<(&NetAbilities, &NetCooldowns)>();
        let (got_abilities, got_cooldowns) = q
            .single(client_app.world())
            .expect("the hotbar mirror reaches the client");
        assert_eq!(*got_abilities, abilities);
        assert_eq!(*got_cooldowns, cooldowns);
    }

    /// `AssignHotbarSlot` travels client -> server (surfacing as
    /// `FromClient<_>`) — the wire-shape half of drag-to-assign for a future
    /// remote client (same posture as EM-5.4/EM-5.8's own wire messages).
    #[test]
    fn assign_hotbar_slot_reaches_server() {
        let mut server_app = new_app();
        let mut client_app = new_app();
        server_app.connect_client(&mut client_app);

        let request = AssignHotbarSlot {
            slot: 2,
            ability: NetAuxiliaryAbility::MainWeapon(1),
        };
        client_app.world_mut().write_message(request);

        client_app.update();
        server_app.exchange_with_client(&mut client_app);
        server_app.update();

        let received: Vec<_> = server_app
            .world_mut()
            .resource_mut::<Messages<FromClient<AssignHotbarSlot>>>()
            .drain()
            .collect();
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].message, request);
    }

    /// [`NetAuxiliaryAbility::to_slot_address_raw`]/[`NetAuxiliaryAbility::
    /// from_slot_address_raw`] round-trip every variant, and two different
    /// indices of the SAME variant never collide (the packing this module's
    /// own doc comment promises the diary's Abilities-tab drag source relies
    /// on).
    #[test]
    fn slot_address_packing_round_trips_every_variant_and_never_collides() {
        let values = [
            NetAuxiliaryAbility::MainWeapon(3),
            NetAuxiliaryAbility::MainWeapon(7),
            NetAuxiliaryAbility::OffWeapon(1),
            NetAuxiliaryAbility::Glider(0),
            NetAuxiliaryAbility::Innate(2),
            NetAuxiliaryAbility::Innate(5),
            NetAuxiliaryAbility::Empty,
        ];
        let mut seen = std::collections::HashSet::new();
        for &aux in &values {
            let raw = aux.to_slot_address_raw();
            assert_eq!(NetAuxiliaryAbility::from_slot_address_raw(raw), aux);
            assert!(seen.insert(raw), "packed address for {aux:?} collided");
        }
    }

    /// A round-trip through RON keeps every `NetAuxiliaryAbility` variant
    /// distinct — cheap regression guard against an accidental serde
    /// collision between variants (used by both the replication test above
    /// and `xindeler-sim-bridge`'s conversion tests).
    #[test]
    fn net_auxiliary_ability_variants_round_trip_distinctly() {
        for aux in [
            NetAuxiliaryAbility::MainWeapon(3),
            NetAuxiliaryAbility::OffWeapon(1),
            NetAuxiliaryAbility::Glider(0),
            NetAuxiliaryAbility::Innate(2),
            NetAuxiliaryAbility::Empty,
        ] {
            let text = ron::ser::to_string(&aux).expect("serializes");
            let back: NetAuxiliaryAbility = ron::from_str(&text).expect("deserializes");
            assert_eq!(back, aux);
        }
    }
}
