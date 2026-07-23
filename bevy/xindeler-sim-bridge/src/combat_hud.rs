//! BL-82 EM-5.2 — the first Phase-5 HUD state-mirror slice (spec
//! `2026-07-11-bl82-phase5-ui-audio-parity-design.md` §3.2/§6): projects the
//! sim's `Energy`/`Poise`/`Combo`/`SkillSet`/`Buffs` onto small replicated
//! `Net*` components the combat HUD reads, following the exact
//! `NetHealth`/`NetLoadout` pattern [`crate::mirror_sim_entities`] already
//! established — but as its OWN, separate, additive system rather than
//! folded into that already ~1000-line function. This is deliberate: the
//! HUD-state mirror is a distinct concern from the entity-lifecycle mirror
//! (spawn/despawn/dimension attribution/region keys) that function owns, and
//! every field here is read-only off storages `mirror_sim_entities` doesn't
//! even open, so there is no shared-borrow reason to live in the same
//! function. It reads [`crate::SimMirror`] (populated THIS tick by
//! `mirror_sim_entities`, which is why this system is ordered
//! `.after(mirror_sim_entities)`) to resolve each currently-mirrored sim
//! entity's Bevy id, then UPSERTs/removes the five new components exactly
//! like `mirror_sim_entities` UPSERTs/removes [`xindeler_protocol::NetHealth`]
//! for entities that do/don't currently have the underlying sim component.
//!
//! Project, don't dump (spec §3.2): every component here is a compact
//! projection, not the sim's real type —
//! [`xindeler_protocol::NetXp`] carries progress-within-level (already
//! subtracted), not the raw lifetime XP total the client would otherwise
//! redo the same arithmetic on every frame for; `NetBuffs` carries one entry
//! per distinct active buff KIND (icon + stack count + remaining duration),
//! not the sim's full `Buff` (effects/source/category bookkeeping stays
//! server-side).

use std::collections::HashMap;

use bevy::{
    app::{App, FixedUpdate, Plugin},
    ecs::{
        change_detection::NonSendMut,
        resource::Resource,
        schedule::IntoScheduleConfigs,
        system::{Commands, Res, ResMut},
    },
};
use common::{comp, comp::skillset::total_exp_for_level, resources::Time as SimTime};
use specs::WorldExt;
use xindeler_protocol::{NetBuffEntry, NetBuffs, NetCombo, NetEnergy, NetPoise, NetXp};

use crate::{SimMirror, SimServer, mirror_sim_entities, tick_sim};

/// Last-mirrored `NetCombo`/`NetXp`/`NetBuffs` per sim entity — the SAME
/// dedup shape `mirror_sim_entities` already uses for `NetLoadout`/`RegionKey`
/// (`SimLoadoutCache`/`SimRegionCache`): re-inserting an UNCHANGED value every
/// tick would still force replicon to treat the component as mutated (a real
/// bandwidth cost for `NetBuffs`, which is `Vec`-shaped like `NetLoadout`, not
/// two floats like `NetHealth`/`NetEnergy`/`NetPoise`). Those three stay on
/// the always-overwrite path deliberately — they legitimately change most
/// ticks, exactly like `NetHealth` does. Entries are pruned here (not by
/// `mirror_sim_entities`) whenever a sim entity drops out of [`SimMirror`],
/// mirroring that struct's own per-tick prune shape without adding a new
/// cross-module removal hook.
#[derive(Resource, Default, Debug)]
pub struct CombatHudMirrorCache {
    combo: HashMap<specs::Entity, NetCombo>,
    xp: HashMap<specs::Entity, NetXp>,
    buffs: HashMap<specs::Entity, NetBuffs>,
}

/// Reads the sim's `Energy`/`Poise`/`Combo`/`SkillSet`/`Buffs` for every
/// currently-mirrored entity ([`SimMirror`]) and UPSERTs the corresponding
/// `Net*` component on its Bevy mirror, removing it when the sim entity no
/// longer carries the underlying component (mirrors `mirror_sim_entities`'s
/// own `Some(h) => insert / None => remove::<NetHealth>()` shape for
/// [`xindeler_protocol::NetHealth`]).
///
/// A no-op (returns immediately) if no [`SimServer`] is booted yet — same
/// early-out `mirror_sim_entities` uses.
pub fn mirror_combat_hud_state(
    sim: Option<NonSendMut<SimServer>>,
    mirror: Res<SimMirror>,
    mut cache: ResMut<CombatHudMirrorCache>,
    mut commands: Commands,
) {
    let Some(sim) = sim else { return };

    // Prune cache entries for sim entities no longer mirrored at all — same
    // "one resource, cleared not left to grow unbounded" posture
    // `SimLoadoutCache`/`SimRegionCache` follow, just scoped to this module
    // instead of `mirror_sim_entities`'s own despawn arm.
    cache
        .combo
        .retain(|entity, _| mirror.0.contains_key(entity));
    cache.xp.retain(|entity, _| mirror.0.contains_key(entity));
    cache
        .buffs
        .retain(|entity, _| mirror.0.contains_key(entity));

    let ecs = sim.server.state().ecs();
    let now = *ecs.read_resource::<SimTime>();

    let energies = ecs.read_storage::<comp::Energy>();
    let poises = ecs.read_storage::<comp::Poise>();
    let combos = ecs.read_storage::<comp::Combo>();
    let skill_sets = ecs.read_storage::<comp::SkillSet>();
    let buffs_storage = ecs.read_storage::<comp::Buffs>();

    for (&sim_entity, &bevy_entity) in mirror.0.iter() {
        let mut ec = commands.entity(bevy_entity);

        // NetEnergy/NetPoise stay on the always-overwrite path (like
        // NetHealth) — they legitimately change most ticks.
        match energies.get(sim_entity) {
            Some(energy) => {
                ec.insert(NetEnergy {
                    current: energy.current(),
                    max: energy.maximum(),
                });
            },
            None => {
                ec.remove::<NetEnergy>();
            },
        }

        match poises.get(sim_entity) {
            Some(poise) => {
                ec.insert(NetPoise {
                    current: poise.current(),
                    max: poise.maximum(),
                });
            },
            None => {
                ec.remove::<NetPoise>();
            },
        }

        match combos.get(sim_entity) {
            Some(combo) => {
                let net_combo = NetCombo {
                    counter: combo.counter(),
                };
                if cache.combo.get(&sim_entity) != Some(&net_combo) {
                    ec.insert(net_combo);
                    cache.combo.insert(sim_entity, net_combo);
                }
            },
            None => {
                ec.remove::<NetCombo>();
                cache.combo.remove(&sim_entity);
            },
        }

        match skill_sets.get(sim_entity) {
            Some(skill_set) => {
                let level = skill_set.character_level();
                let total_exp = skill_set.total_earned_exp();
                let level_floor = total_exp_for_level(level);
                let next_floor = total_exp_for_level(level.saturating_add(1));
                let net_xp = NetXp {
                    level,
                    xp_into_level: total_exp.saturating_sub(level_floor),
                    xp_for_level: next_floor.saturating_sub(level_floor),
                };
                if cache.xp.get(&sim_entity) != Some(&net_xp) {
                    ec.insert(net_xp);
                    cache.xp.insert(sim_entity, net_xp);
                }
            },
            None => {
                ec.remove::<NetXp>();
                cache.xp.remove(&sim_entity);
            },
        }

        match buffs_storage.get(sim_entity) {
            Some(buffs) => {
                let mut entries = Vec::new();
                for (kind, slot) in buffs.kinds.iter() {
                    if slot.is_none() {
                        continue;
                    }
                    let Some((_, controlling)) = buffs.iter_kind(kind).next() else {
                        continue;
                    };
                    let stacks = buffs.iter_kind(kind).count() as u32;
                    let remaining_secs = controlling
                        .end_time
                        .map(|end| (end.0 - now.0).max(0.0) as f32);
                    entries.push(NetBuffEntry {
                        kind,
                        strength: controlling.data.strength,
                        remaining_secs,
                        stacks,
                    });
                }
                let net_buffs = NetBuffs(entries);
                if cache.buffs.get(&sim_entity) != Some(&net_buffs) {
                    ec.insert(net_buffs.clone());
                    cache.buffs.insert(sim_entity, net_buffs);
                }
            },
            None => {
                ec.remove::<NetBuffs>();
                cache.buffs.remove(&sim_entity);
            },
        }
    }
}

/// Registers [`mirror_combat_hud_state`] in `FixedUpdate`, after
/// `mirror_sim_entities` (so [`SimMirror`] is this tick's up-to-date
/// sim↔Bevy identity map before we resolve entities against it) and after
/// `tick_sim` (so the sim state we read is this tick's fresh values, matching
/// every sibling mirror system's own ordering). Add alongside
/// [`crate::SimEntityMirrorPlugin`] (after it, per this plugin's own doc
/// comment) in whichever shell hosts the bridge (the listen-server client and
/// `xindeler-server-app`'s dedicated-server shell both do).
pub struct CombatHudMirrorPlugin;

impl Plugin for CombatHudMirrorPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CombatHudMirrorCache>().add_systems(
            FixedUpdate,
            mirror_combat_hud_state
                .after(tick_sim)
                .after(mirror_sim_entities),
        );
    }
}

#[cfg(test)]
mod tests {
    use bevy::{app::App, ecs::system::RunSystemOnce, prelude::MinimalPlugins};
    use common::{comp::buff::BuffKind, resources::Time};
    use specs::{Builder, WorldExt};
    use xindeler_protocol::{NetBuffs, NetCombo, NetEnergy, NetPoise, NetXp};

    use super::*;
    use crate::{SimServer, boot_test_server};

    /// A minimal Bevy app with a booted [`SimServer`] + an empty [`SimMirror`]
    /// — enough to exercise [`mirror_combat_hud_state`] directly via
    /// `run_system_once`, mirroring the crate's other direct-system-call
    /// tests (e.g. `narrative`'s `fire_on_enter_toasts` tests upstream).
    fn new_app_with_sim(data_dir: &std::path::Path) -> App {
        let sim = boot_test_server(data_dir).expect("test server boots");
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<SimMirror>();
        app.init_resource::<CombatHudMirrorCache>();
        app.insert_non_send(sim);
        app
    }

    /// `mirror_combat_hud_state` is a documented no-op (no panic, nothing
    /// written) when [`SimMirror`] is empty (nothing mirrored yet) — the
    /// "degrade clean" rule (spec §3.2).
    #[test]
    fn no_mirrored_entities_is_a_harmless_no_op() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = new_app_with_sim(dir.path());

        app.world_mut()
            .run_system_once(mirror_combat_hud_state)
            .expect("system runs without a mirrored entity");
    }

    /// A sim entity carrying `Energy`/`Poise`/`Combo`/`SkillSet`/`Buffs`,
    /// once registered in `SimMirror` against a real Bevy entity, gets all
    /// five `Net*` components UPSERTed with the projected values — the core
    /// acceptance bar for this mirror slice (spec §6: "prove the pattern
    /// end-to-end on a small screen").
    #[test]
    fn mirrors_energy_poise_combo_xp_and_buffs_for_a_real_sim_entity() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = new_app_with_sim(dir.path());

        // Build a real sim entity carrying the five source components,
        // directly on the booted `SimServer`'s specs world (the same access
        // pattern `mirror_sim_entities`'s own tests use for fixture setup).
        let sim_entity = {
            let mut sim = app.world_mut().non_send_mut::<SimServer>();
            let ecs = sim.server.state_mut().ecs_mut();
            let now = *ecs.read_resource::<Time>();
            let body = comp::Body::Humanoid(comp::humanoid::Body::random());

            let mut buffs = comp::Buffs::default();
            buffs.insert(
                comp::Buff::new(
                    BuffKind::Regeneration,
                    comp::BuffData::new(3.0, Some(common::resources::Secs(10.0))),
                    Vec::new(),
                    comp::BuffSource::World,
                    now,
                    comp::buff::DestInfo {
                        stats: None,
                        mass: None,
                    },
                    None,
                    None,
                ),
                now,
            );

            ecs.create_entity()
                .with(comp::Energy::new(body))
                .with(comp::Poise::new(body))
                .with(comp::Combo::default())
                .with(comp::SkillSet::default())
                .with(buffs)
                .build()
        };

        // Wire up a fake Bevy mirror entity + the SimMirror map entry, the
        // way `mirror_sim_entities` itself would have on a prior UPSERT arm.
        let bevy_entity = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<SimMirror>()
            .0
            .insert(sim_entity, bevy_entity);

        app.world_mut()
            .run_system_once(mirror_combat_hud_state)
            .expect("system runs");
        app.update();

        let world = app.world();
        assert!(
            world.get::<NetEnergy>(bevy_entity).is_some(),
            "NetEnergy must be mirrored"
        );
        assert!(
            world.get::<NetPoise>(bevy_entity).is_some(),
            "NetPoise must be mirrored"
        );
        let combo = world
            .get::<NetCombo>(bevy_entity)
            .expect("NetCombo must be mirrored");
        assert_eq!(combo.counter, 0);
        let xp = world
            .get::<NetXp>(bevy_entity)
            .expect("NetXp must be mirrored");
        assert_eq!(xp.level, 1, "a fresh SkillSet starts at level 1");
        let buffs = world
            .get::<NetBuffs>(bevy_entity)
            .expect("NetBuffs must be mirrored");
        assert_eq!(
            buffs.0.len(),
            1,
            "the one inserted Regeneration buff mirrors"
        );
        assert_eq!(buffs.0[0].kind, BuffKind::Regeneration);
        assert_eq!(buffs.0[0].stacks, 1);
    }

    /// A `BuffKind` that permits multiple simultaneous instances
    /// (`BuffKind::stacks() == true`, e.g. `Resilience`) mirrors as ONE
    /// `NetBuffEntry` with `stacks` counting every live instance — the
    /// ecs-design-reviewer's flagged gap: the single-buff test above can't
    /// tell "stacks == 1 because there's one buff" from "stacks == 1 because
    /// counting is broken".
    #[test]
    fn multiple_stacked_instances_of_the_same_kind_report_the_real_stack_count() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = new_app_with_sim(dir.path());

        let sim_entity = {
            let mut sim = app.world_mut().non_send_mut::<SimServer>();
            let ecs = sim.server.state_mut().ecs_mut();
            let now = *ecs.read_resource::<Time>();

            let mut buffs = comp::Buffs::default();
            for _ in 0..3 {
                buffs.insert(
                    comp::Buff::new(
                        BuffKind::Resilience,
                        comp::BuffData::new(1.0, Some(common::resources::Secs(10.0))),
                        Vec::new(),
                        comp::BuffSource::World,
                        now,
                        comp::buff::DestInfo {
                            stats: None,
                            mass: None,
                        },
                        None,
                        None,
                    ),
                    now,
                );
            }

            ecs.create_entity().with(buffs).build()
        };

        let bevy_entity = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<SimMirror>()
            .0
            .insert(sim_entity, bevy_entity);

        app.world_mut()
            .run_system_once(mirror_combat_hud_state)
            .expect("system runs");
        app.update();

        let buffs = app
            .world()
            .get::<NetBuffs>(bevy_entity)
            .expect("NetBuffs must be mirrored");
        assert_eq!(
            buffs.0.len(),
            1,
            "one entry per DISTINCT kind, not per instance"
        );
        assert_eq!(buffs.0[0].kind, BuffKind::Resilience);
        assert_eq!(
            buffs.0[0].stacks, 3,
            "stacks must count all 3 live instances of the stacking kind"
        );
    }

    /// An entity that LOSES its sim-side `Buffs`/`Energy`/`Poise`/`SkillSet`
    /// components (not just an empty `Buffs`, but the component removed
    /// outright) has its corresponding `Net*` mirror component removed too —
    /// the `None => ec.remove::<NetX>()` arms, which the single-entity
    /// insert-path tests above never exercised (ecs-design-reviewer finding).
    #[test]
    fn removing_a_sim_component_removes_its_net_mirror_too() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = new_app_with_sim(dir.path());

        let sim_entity = {
            let mut sim = app.world_mut().non_send_mut::<SimServer>();
            let ecs = sim.server.state_mut().ecs_mut();
            let body = comp::Body::Humanoid(comp::humanoid::Body::random());
            ecs.create_entity()
                .with(comp::Energy::new(body))
                .with(comp::SkillSet::default())
                .build()
        };

        let bevy_entity = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<SimMirror>()
            .0
            .insert(sim_entity, bevy_entity);

        app.world_mut()
            .run_system_once(mirror_combat_hud_state)
            .expect("first run mirrors NetEnergy/NetXp");
        app.update();
        assert!(app.world().get::<NetEnergy>(bevy_entity).is_some());
        assert!(
            app.world()
                .get::<xindeler_protocol::NetXp>(bevy_entity)
                .is_some()
        );

        // Remove the sim-side components (as if the entity, e.g., turned into
        // a pure prop/corpse that no longer has energy or a skillset).
        {
            let mut sim = app.world_mut().non_send_mut::<SimServer>();
            let ecs = sim.server.state_mut().ecs_mut();
            ecs.write_storage::<comp::Energy>().remove(sim_entity);
            ecs.write_storage::<comp::SkillSet>().remove(sim_entity);
        }

        app.world_mut()
            .run_system_once(mirror_combat_hud_state)
            .expect("second run must remove the now-stale mirrors");
        app.update();

        assert!(
            app.world().get::<NetEnergy>(bevy_entity).is_none(),
            "NetEnergy must be removed once the sim-side Energy is gone"
        );
        assert!(
            app.world()
                .get::<xindeler_protocol::NetXp>(bevy_entity)
                .is_none(),
            "NetXp must be removed once the sim-side SkillSet is gone"
        );
    }
}
