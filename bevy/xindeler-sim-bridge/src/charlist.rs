//! BL-82 EM-5.14 (T56.32) — the character-list bridge: projects the embedded
//! player's real `client::Client` character roster onto the wire
//! ([`NetCharList`]) and applies the player's create/delete/select intents
//! ([`LocalCharCreate`]/[`LocalCharDelete`]/[`LocalCharSelect`]) back onto that
//! same Client.
//!
//! Like [`crate::chat`] (and unlike the per-entity `Net*` component mirrors),
//! this revolves around the [`EmbeddedPlayer`]'s `client::Client`, not
//! [`crate::SimMirror`] — the character roster is account-scoped data with no
//! mirrored world entity (spec §3.2's "bulk data = messages" rule). So both
//! systems run in `Update`, `.after(tick_player)` (the roster lives in the
//! embedded Client, refreshed by `client.tick()` inside
//! [`crate::player::tick_player`]), and the roster is broadcast change-deduped
//! (only when it actually changes — create/delete/relog), the same posture
//! [`crate::social::mirror_player_list`] uses.
//!
//! ## Known gap: listen-server only (same posture as [`crate::chat`])
//! [`CharListMirrorPlugin`] is added only by `xindeler-client::listen_server`,
//! and only when launched into the char-select screen. `SendTargets::All` is
//! safe because a listen-server has exactly one embedded player; a real
//! dedicated-server char-select would need a per-session roster keyed by the
//! connecting client (the `CharCreateRequest`/`CharDeleteRequest`/
//! `CharSelectRequest` wire twins exist and are registered for exactly that
//! future path, but nothing consumes their `FromClient<_>` form yet).

use bevy::{
    app::{App, Plugin, Update},
    ecs::{
        change_detection::{NonSend, NonSendMut},
        message::{MessageReader, MessageWriter},
        resource::Resource,
        schedule::IntoScheduleConfigs,
        system::ResMut,
    },
};
use bevy_replicon::prelude::{SendTargets, ToClients};
use xindeler_protocol::{
    LocalCharCreate, LocalCharDelete, LocalCharSelect, NetCharList, NetCharListEntry,
};

use crate::{EmbeddedPlayer, player::tick_player};

/// Last-broadcast [`NetCharList`], so an UNCHANGED roster (the common case,
/// every frame the player sits on the select screen) never forces a wire send
/// — the same dedup posture [`crate::social::PlayerListCache`] uses.
#[derive(Resource, Default, Debug)]
pub struct CharListMirrorCache(Option<NetCharList>);

/// Projects the embedded Client's `CharacterList` onto the wire [`NetCharList`]
/// shape. Only characters with a real persisted id are included (a roster
/// entry mid-creation may briefly have `id == None`); the alias/body/hardcore/
/// location come straight off the `CharacterItem`.
fn project_char_list(list: &client::CharacterList) -> NetCharList {
    let characters = list
        .characters
        .iter()
        .filter_map(|item| {
            item.character.id.map(|id| NetCharListEntry {
                id,
                alias: item.character.alias.clone(),
                body: item.body,
                hardcore: item.hardcore,
                location: item.location.clone(),
            })
        })
        .collect();
    NetCharList {
        characters,
        loading: list.loading,
    }
}

/// Broadcasts [`NetCharList`] whenever the embedded player's roster changes
/// (`SendTargets::All` — see the module doc comment). A no-op until an
/// [`EmbeddedPlayer`] exists; the roster it reads is populated as soon as one
/// does (`boot_embedded_player` kicks off the load).
pub fn broadcast_char_list(
    player: Option<NonSend<EmbeddedPlayer>>,
    mut cache: ResMut<CharListMirrorCache>,
    mut writer: MessageWriter<ToClients<NetCharList>>,
) {
    let Some(player) = player else { return };
    let projected = project_char_list(player.character_list());
    if cache.0.as_ref() != Some(&projected) {
        cache.0 = Some(projected.clone());
        writer.write(ToClients {
            targets: SendTargets::All,
            message: projected,
        });
    }
}

/// Drains the three in-process char-select intents (the listen-server's
/// client→bridge handoff — see `xindeler_protocol::charlist`'s module doc
/// comment) and forwards each to the matching [`EmbeddedPlayer`] pass-through
/// (a genuine network send over the loopback socket, never a direct sim
/// write). A no-op (messages dropped) until an [`EmbeddedPlayer`] exists.
pub fn apply_char_requests(
    player: Option<NonSendMut<EmbeddedPlayer>>,
    mut creates: MessageReader<LocalCharCreate>,
    mut deletes: MessageReader<LocalCharDelete>,
    mut selects: MessageReader<LocalCharSelect>,
) {
    let Some(mut player) = player else {
        creates.clear();
        deletes.clear();
        selects.clear();
        return;
    };
    for LocalCharCreate(params) in creates.read() {
        player.submit_create_character(params);
    }
    for LocalCharDelete(id) in deletes.read() {
        player.delete_character(*id);
    }
    for LocalCharSelect(id) in selects.read() {
        player.select_character(*id);
    }
}

/// Registers the EM-5.14 char-list mirror + intent applicator in `Update`,
/// `.after(tick_player)` (see the module doc comment for why this schedule, not
/// `FixedUpdate`/`tick_sim`). Add alongside [`crate::PlayerBridgePlugin`]
/// (after it — same convention [`crate::ChatBridgePlugin`] follows) in the
/// listen-server shell, and only when launched into the char-select screen.
pub struct CharListMirrorPlugin;

impl Plugin for CharListMirrorPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CharListMirrorCache>().add_systems(
            Update,
            // `.chain()`: apply this frame's intent first, then project the
            // roster — matching `SocialMirrorPlugin`'s own documented ordering
            // (both borrow `EmbeddedPlayer`, one `&mut`, one `&`).
            (apply_char_requests, broadcast_char_list)
                .chain()
                .after(tick_player),
        );
    }
}

#[cfg(test)]
mod tests {
    use bevy::app::App;
    use xindeler_protocol::CharCreateParams;

    use super::*;

    fn humanoid_body() -> common::comp::Body {
        common::comp::Body::Humanoid(common::comp::humanoid::Body {
            species: common::comp::humanoid::Species::Human,
            body_type: common::comp::humanoid::BodyType::Male,
            hair_style: 0,
            beard: 0,
            eyes: 0,
            accessory: 0,
            hair_color: 0,
            skin: 0,
            eye_color: 0,
            height_scale: 0,
        })
    }

    /// [`project_char_list`]: a real `CharacterItem` (with a persisted id) maps
    /// verbatim; the `loading` flag rides through. No assets, no sim.
    #[test]
    fn project_char_list_maps_persisted_characters() {
        // `CharacterList::default()` already has `loading: false`.
        let mut list = client::CharacterList::default();
        list.characters.push(common::character::CharacterItem {
            character: common::character::Character {
                id: Some(common::character::CharacterId(9)),
                alias: "Kaelis".to_owned(),
            },
            body: humanoid_body(),
            hardcore: true,
            inventory: common::comp::Inventory::with_empty(),
            location: Some("Emberfall".to_owned()),
        });

        let net = project_char_list(&list);
        assert!(!net.loading);
        assert_eq!(net.characters.len(), 1);
        assert_eq!(net.characters[0].id, common::character::CharacterId(9));
        assert_eq!(net.characters[0].alias, "Kaelis");
        assert!(net.characters[0].hardcore);
        assert_eq!(net.characters[0].location.as_deref(), Some("Emberfall"));
    }

    /// [`project_char_list`]: an entry still mid-creation (no persisted id) is
    /// filtered out — the select screen never shows a half-created row.
    #[test]
    fn project_char_list_skips_id_less_entries() {
        let mut list = client::CharacterList::default();
        list.characters.push(common::character::CharacterItem {
            character: common::character::Character {
                id: None,
                alias: "Pending".to_owned(),
            },
            body: humanoid_body(),
            hardcore: false,
            inventory: common::comp::Inventory::with_empty(),
            location: None,
        });
        assert!(project_char_list(&list).characters.is_empty());
    }

    /// [`apply_char_requests`] runs (and drains its readers) even with no
    /// [`EmbeddedPlayer`] present — it must never panic on a request that
    /// arrives before the world has finished booting (mirrors
    /// `apply_local_group_actions`'s own no-player early-out). Running it a
    /// second time proves the readers were advanced (no re-processing): the
    /// system's `creates.clear()`/etc. moved each reader's cursor past the
    /// messages, so the second run sees nothing new. (`Messages::drain()` would
    /// NOT be `0` here — Bevy double-buffers, so the buffer keeps the messages
    /// for one more `update()` cycle regardless of the reader cursor.)
    #[test]
    fn apply_char_requests_runs_without_player() {
        use bevy::ecs::system::RunSystemOnce;

        let mut app = App::new();
        app.add_message::<LocalCharCreate>();
        app.add_message::<LocalCharDelete>();
        app.add_message::<LocalCharSelect>();

        app.world_mut()
            .write_message(LocalCharCreate(CharCreateParams {
                alias: "Aria".to_owned(),
                mainhand: None,
                offhand: None,
                body: humanoid_body(),
                hardcore: false,
                class: common::comp::ClassKind::Warrior,
                ethos: common::comp::Ethos::default(),
                background: common::comp::Background::default(),
            }));
        app.world_mut()
            .write_message(LocalCharDelete(common::character::CharacterId(1)));
        app.world_mut()
            .write_message(LocalCharSelect(common::character::CharacterId(1)));

        // No `EmbeddedPlayer` inserted — must not panic.
        app.world_mut()
            .run_system_once(apply_char_requests)
            .expect("system runs with no player");
        // A second run with no new messages must also be a clean no-op.
        app.world_mut()
            .run_system_once(apply_char_requests)
            .expect("system runs again");
    }

    // ---- Heavy end-to-end round-trip (real sim + embedded player) -----------
    // These boot the REAL sim + a manual-selection embedded player exactly as
    // `--listen-server --char-select` does, drive a create/delete intent
    // through the SAME `LocalChar*` → `apply_char_requests` → embedded
    // `client::Client` path the UI uses, and assert on the server-authoritative
    // roster. `#[ignore]` (needs assets + LFS); run with VELOREN_ASSETS.

    use std::time::Duration;

    use bevy::{MinimalPlugins, app::PluginGroup, state::app::StatesPlugin};
    use bevy_replicon::prelude::{RepliconPlugins, ServerPlugin};
    use xindeler_protocol::XindelerProtocolPlugin;

    use crate::{
        PlayerBridgePlugin, SimBridgePlugin, SimEntityMirrorPlugin, boot_embedded_player,
        boot_test_server,
    };

    /// Sleeps a realistic per-frame interval before `app.update()` so the
    /// embedded player's wall-clock `Clock` smoother sees a sane dt (the same
    /// reason `player.rs`'s own heavy tests do this — see its
    /// `update_with_real_frame_time`).
    fn step(app: &mut App) {
        std::thread::sleep(Duration::from_secs_f64(1.0 / crate::SIM_TICK_HZ));
        app.update();
    }

    fn build_app(sim: crate::SimServer, player: EmbeddedPlayer) -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins.build())
            .add_plugins(StatesPlugin)
            .add_plugins(RepliconPlugins.set(ServerPlugin::new(bevy::app::PostUpdate)))
            .add_plugins((
                XindelerProtocolPlugin,
                SimBridgePlugin,
                SimEntityMirrorPlugin,
                PlayerBridgePlugin,
                CharListMirrorPlugin,
            ))
            .finish();
        // `LocalPlayerInput` and `Time` are already inserted by
        // `PlayerBridgePlugin`/`MinimalPlugins` respectively.
        app.insert_non_send(sim);
        app.insert_non_send(player);
        app
    }

    /// T56.32 core: a player-driven `create` persists AND enters the world.
    /// Boots manual, waits until the machine parks awaiting selection with an
    /// EMPTY roster, submits a `LocalCharCreate` (Mage + starter staff) exactly
    /// as the wizard's Create button does, then ticks until in-game and asserts
    /// the server-authoritative roster now holds that one character. Because
    /// the roster is rebuilt from the persistence layer, its presence is
    /// proof the character persisted.
    #[test]
    #[ignore = "boots a real world + embedded player: needs assets + LFS; run with VELOREN_ASSETS"]
    fn char_create_persists_and_enters_world() {
        const MAX_TICKS: u32 = 8000;

        let data_dir = tempfile::tempdir().expect("tempdir");
        let mut sim = boot_test_server(data_dir.path()).expect("test server boots");
        let mut player = boot_embedded_player(&mut sim).expect("embedded player boots");
        player.set_manual_selection(true);
        let mut app = build_app(sim, player);

        // Park awaiting selection with an empty roster.
        let mut awaited = false;
        for _ in 0..MAX_TICKS {
            step(&mut app);
            let p = app.world().non_send::<EmbeddedPlayer>();
            if p.is_awaiting_selection() {
                assert!(
                    project_char_list(p.character_list()).characters.is_empty(),
                    "a fresh account starts with no characters"
                );
                awaited = true;
                break;
            }
        }
        assert!(awaited, "embedded player never parked awaiting selection");

        // Submit the creation wizard's result.
        app.world_mut()
            .write_message(LocalCharCreate(CharCreateParams {
                alias: "Elowen".to_owned(),
                mainhand: Some("common.items.weapons.staff.starter_staff".to_owned()),
                offhand: None,
                body: humanoid_body(),
                hardcore: false,
                class: common::comp::ClassKind::Mage,
                ethos: common::comp::Ethos::default(),
                background: common::comp::Background::default(),
            }));

        // Tick until in-game.
        let mut in_game = false;
        for _ in 0..MAX_TICKS {
            step(&mut app);
            if app.world().non_send::<EmbeddedPlayer>().is_in_game() {
                in_game = true;
                break;
            }
        }
        assert!(in_game, "created character never entered the world");

        let roster = project_char_list(app.world().non_send::<EmbeddedPlayer>().character_list());
        assert_eq!(roster.characters.len(), 1, "exactly the created character");
        assert_eq!(
            roster.characters[0].alias, "Elowen",
            "the created alias persisted"
        );
    }

    /// T56.32 delete: after a character is persisted (created in a first
    /// session, above), a fresh manual session loads it into the roster and a
    /// `LocalCharDelete` removes it. Uses a relog (drop the first embedded
    /// player, boot a second against the same persisted data dir) because a
    /// delete happens from the select screen, not in-game.
    #[test]
    #[ignore = "boots a real world + embedded player: needs assets + LFS; run with VELOREN_ASSETS"]
    fn char_delete_removes_from_roster() {
        const MAX_TICKS: u32 = 8000;

        let data_dir = tempfile::tempdir().expect("tempdir");

        // --- Session 1: create + persist a character, then drop it. ---
        {
            let mut sim = boot_test_server(data_dir.path()).expect("test server boots");
            let mut player = boot_embedded_player(&mut sim).expect("embedded player boots");
            player.set_manual_selection(true);
            let mut app = build_app(sim, player);

            for _ in 0..MAX_TICKS {
                step(&mut app);
                if app
                    .world()
                    .non_send::<EmbeddedPlayer>()
                    .is_awaiting_selection()
                {
                    break;
                }
            }
            app.world_mut()
                .write_message(LocalCharCreate(CharCreateParams {
                    alias: "Doomed".to_owned(),
                    mainhand: Some("common.items.weapons.sword.starter".to_owned()),
                    offhand: None,
                    body: humanoid_body(),
                    hardcore: false,
                    class: common::comp::ClassKind::Warrior,
                    ethos: common::comp::Ethos::default(),
                    background: common::comp::Background::default(),
                }));
            for _ in 0..MAX_TICKS {
                step(&mut app);
                if app.world().non_send::<EmbeddedPlayer>().is_in_game() {
                    break;
                }
            }
            assert!(
                app.world().non_send::<EmbeddedPlayer>().is_in_game(),
                "session 1 character never entered world / persisted"
            );
            // Let persistence settle before dropping the session.
            for _ in 0..120 {
                step(&mut app);
            }
        }

        // --- Session 2: relog, load the persisted roster, delete it. ---
        let mut sim = boot_test_server(data_dir.path()).expect("test server re-boots");
        let mut player = boot_embedded_player(&mut sim).expect("embedded player re-boots");
        player.set_manual_selection(true);
        let mut app = build_app(sim, player);

        // Wait until the persisted character shows in the roster.
        let mut saw_char = false;
        for _ in 0..MAX_TICKS {
            step(&mut app);
            let p = app.world().non_send::<EmbeddedPlayer>();
            if p.is_awaiting_selection()
                && !project_char_list(p.character_list()).characters.is_empty()
            {
                saw_char = true;
                break;
            }
        }
        assert!(saw_char, "session 2 never loaded the persisted character");

        let id = project_char_list(app.world().non_send::<EmbeddedPlayer>().character_list())
            .characters[0]
            .id;
        app.world_mut().write_message(LocalCharDelete(id));

        // Tick until the roster empties.
        let mut deleted = false;
        for _ in 0..MAX_TICKS {
            step(&mut app);
            let p = app.world().non_send::<EmbeddedPlayer>();
            if project_char_list(p.character_list()).characters.is_empty() {
                deleted = true;
                break;
            }
        }
        assert!(
            deleted,
            "delete never removed the character from the roster"
        );
    }
}
