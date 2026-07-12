//! BL-82 EM-5.7 — the character diary / skill-tree mirror + SP-spend request
//! (spec §2/§3.2, tasks T56.22-.24).
//!
//! ## Reusing BL-06's data-driven philosophy — what does NOT need a mirror
//! The legacy `voxygen` diary's generic class-tree renderer
//! (`handle_class_skills_window`, `voxygen/src/hud/diary.rs`) reads the
//! STATIC tree shape — which skills belong to a group
//! (`common::comp::skillset::SKILL_GROUP_DEFS`), each skill's prerequisites
//! (`SKILL_PREREQUISITES`), its max level (`SKILL_MAX_LEVEL`) and passive
//! stat modifiers (`CLASS_SKILL_MODIFIERS`/`FEAT_MODIFIERS`) — directly out of
//! `common`'s `lazy_static`s (RON-loaded from `VELOREN_ASSETS` at first
//! access). The Bevy client already links `common` as a type library (the
//! SAME isolation-law carve-out `NetBuffEntry` uses to reuse `BuffKind`
//! verbatim below) and already resolves `VELOREN_ASSETS`/`XINDELER_ASSETS` at
//! runtime (`xindeler-ui::i18n`'s own `assets_root()`) — so
//! `xindeler-client::diary` reads those same `common` statics DIRECTLY,
//! client-side, rather than this crate replicating the whole tree shape over
//! the wire. Only the DYNAMIC per-player state — which skill groups exist,
//! their SP counts, and which skills are unlocked at which level — is a
//! genuine "project, don't dump" wire projection: [`NetSkillSet`].
//!
//! [`NetAbilityPool`] is the parallel mirror for the diary's **Abilities**
//! tab: the FULL set of abilities the player currently qualifies for
//! (`common::comp::ability::ActiveAbilities::all_available_abilities` — main
//! weapon / off weapon / glider / every unlocked innate), as opposed to
//! [`crate::hotbar::NetAbilities`] (EM-5.3), which only carries the handful
//! CURRENTLY BOUND to a hotbar slot. Reuses [`crate::hotbar::NetHotbarSlot`]'s
//! exact `{aux, ability_id}` shape (no new wire type needed) — dragging one of
//! these entries onto the hotbar is the ability-drag T56.24 wires up
//! (`xindeler-client::diary`'s Abilities tab is the drag SOURCE,
//! `xindeler-client::hotbar`'s existing drop handler is the target, both over
//! `xindeler_ui::slot::SlotDropped`).
//!
//! ## The write half: real SP-spend, the listen-server posture
//! [`UnlockSkillRequest`] is the real replicon client message (wire shape for
//! a FUTURE genuinely-remote client, registered like every other Phase-5 wire
//! message) — dormant today, same posture as `AssignHotbarSlot`.
//! [`LocalUnlockSkillRequest`] is what actually drives gameplay: a plain
//! in-process Bevy message the diary UI writes directly, drained by
//! `xindeler-sim-bridge::skillset::apply_local_skill_unlock_requests`, which
//! calls the embedded player's real `client::Client::unlock_skill` — a
//! genuine client->server network send over the loopback socket (that method
//! already existed on `client::Client`, sending the SAME `ClientGeneral::
//! UnlockSkill` the legacy diary's `Event::UnlockSkill` handler sends —
//! `server/src/sys/msg/in_game.rs` already processes it, no server-side
//! change needed), never a direct ECS write (isolation-law rule 4) — exactly
//! EM-5.3's `LocalAssignHotbarSlot`/`apply_local_hotbar_assignment`
//! precedent.

use bevy::ecs::{component::Component, message::Message};
use common::comp::skillset::{SkillGroupKind, skills::Skill};
use serde::{Deserialize, Serialize};

use crate::hotbar::NetHotbarSlot;

/// One skill group's dynamic SP state (BL-82 EM-5.7). `kind` reuses
/// `common::comp::skillset::SkillGroupKind` verbatim (it's `Copy` +
/// `Serialize`/`Deserialize` already) — the SAME "reuse the sim's own small
/// enum directly rather than mint a `NetX` twin" precedent `NetBuffEntry`
/// already established for `BuffKind`.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct NetSkillGroup {
    pub kind: SkillGroupKind,
    pub available_sp: u16,
    pub earned_sp: u16,
}

/// Replicated skillset snapshot (BL-82 EM-5.7): every unlocked skill group's
/// SP state + every unlocked skill's level. See this module's own doc comment
/// for why the STATIC tree shape (prereqs/costs/tiers/group membership) is
/// deliberately NOT part of this — it's read directly from `common`'s
/// RON-backed statics client-side instead.
#[derive(Component, Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct NetSkillSet {
    pub groups: Vec<NetSkillGroup>,
    /// `(skill, level)` for every skill with `level > 0` — reuses `Skill`
    /// verbatim (`Copy` + `Serialize`/`Deserialize`, same reuse precedent as
    /// `NetSkillGroup::kind` above).
    pub skills: Vec<(Skill, u16)>,
}

/// Replicated "every ability the player currently qualifies for" snapshot
/// (BL-82 EM-5.7) — the diary's Abilities-tab drag source. See this module's
/// own doc comment for how this differs from [`crate::hotbar::NetAbilities`].
#[derive(Component, Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct NetAbilityPool(pub Vec<NetHotbarSlot>);

/// Client -> server: spend a skill point unlocking `0` (BL-82 EM-5.7 SP-spend
/// action). The real replicon wire shape for a FUTURE genuinely-remote
/// client — see this module's own doc comment for why the listen-server path
/// does not read this today ([`LocalUnlockSkillRequest`] does).
#[derive(Message, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnlockSkillRequest(pub Skill);

/// The in-process listen-server counterpart of [`UnlockSkillRequest`] — see
/// this module's own doc comment.
#[derive(Message, Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalUnlockSkillRequest(pub Skill);

#[cfg(test)]
mod tests {
    use bevy::{app::App, ecs::message::Messages, prelude::*};
    use bevy_replicon::{
        prelude::{FromClient, RepliconPlugins, ServerPlugin},
        test_app::ServerTestAppExt,
    };
    use common::comp::skillset::skills::WarriorSkill;

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

    /// BL-82 EM-5.7 acceptance (T56.22 mirror half): `NetSkillSet` round-trips
    /// server -> client exactly like every other `Net*` mirror (spec §3.2).
    #[test]
    fn skillset_mirror_replicates() {
        use bevy_replicon::prelude::Replicated;

        let mut server_app = new_app();
        let mut client_app = new_app();
        server_app.connect_client(&mut client_app);

        let skillset = NetSkillSet {
            groups: vec![
                NetSkillGroup {
                    kind: SkillGroupKind::General,
                    available_sp: 2,
                    earned_sp: 5,
                },
                NetSkillGroup {
                    kind: SkillGroupKind::Class(common::comp::class::ClassKind::Warrior),
                    available_sp: 1,
                    earned_sp: 1,
                },
            ],
            skills: vec![
                (Skill::UnlockGroup(SkillGroupKind::General), 1),
                (Skill::Warrior(WarriorSkill::Rally), 1),
            ],
        };

        server_app.world_mut().spawn((Replicated, skillset.clone()));

        server_app.update();
        server_app.exchange_with_client(&mut client_app);
        client_app.update();

        let mut q = client_app.world_mut().query::<&NetSkillSet>();
        let got = q
            .single(client_app.world())
            .expect("the skillset mirror reaches the client");
        assert_eq!(*got, skillset);
    }

    /// `NetAbilityPool` round-trips server -> client the same way, carrying
    /// more than [`crate::hotbar::NetAbilities`]'s current-slot count would
    /// (the diary's Abilities tab shows every qualifying ability, not just
    /// the bound hotbar slots).
    #[test]
    fn ability_pool_mirror_replicates() {
        use crate::hotbar::NetAuxiliaryAbility;
        use bevy_replicon::prelude::Replicated;

        let mut server_app = new_app();
        let mut client_app = new_app();
        server_app.connect_client(&mut client_app);

        let pool = NetAbilityPool(vec![
            NetHotbarSlot {
                aux: NetAuxiliaryAbility::MainWeapon(0),
                ability_id: Some("common.abilities.sword.m1".to_owned()),
            },
            NetHotbarSlot {
                aux: NetAuxiliaryAbility::Innate(0),
                ability_id: Some("class.warrior.rally".to_owned()),
            },
        ]);

        server_app.world_mut().spawn((Replicated, pool.clone()));

        server_app.update();
        server_app.exchange_with_client(&mut client_app);
        client_app.update();

        let mut q = client_app.world_mut().query::<&NetAbilityPool>();
        let got = q
            .single(client_app.world())
            .expect("the ability-pool mirror reaches the client");
        assert_eq!(*got, pool);
    }

    /// `UnlockSkillRequest` travels client -> server (surfacing as
    /// `FromClient<_>`) — the wire-shape half of SP-spend for a future remote
    /// client (same posture as every other Phase-5 request message).
    #[test]
    fn unlock_skill_request_reaches_server() {
        let mut server_app = new_app();
        let mut client_app = new_app();
        server_app.connect_client(&mut client_app);

        let request = UnlockSkillRequest(Skill::Warrior(WarriorSkill::Rally));
        client_app.world_mut().write_message(request);

        client_app.update();
        server_app.exchange_with_client(&mut client_app);
        server_app.update();

        let received: Vec<_> = server_app
            .world_mut()
            .resource_mut::<Messages<FromClient<UnlockSkillRequest>>>()
            .drain()
            .collect();
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].message, request);
    }
}
