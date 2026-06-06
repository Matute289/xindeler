use common::comp::{Body, CharacterState, Energy, Health, Player, Pos, Vel};
use common::resources::TimeOfDay;
use common_ecs::{Job, Origin, Phase, System};
use specs::{Join, Read, ReadStorage, SystemData, shred};
use std::sync::atomic::{AtomicU32, Ordering};

const SNAPSHOT_TICKS: u32 = 150;
const EC_RADIUS_SQ: f32 = 40.0 * 40.0;
const EC_MAX_ENTITIES: usize = 20;

static TICK_COUNTER: AtomicU32 = AtomicU32::new(0);

#[derive(Default)]
pub struct Sys;

#[derive(SystemData)]
pub struct ReadData<'a> {
    time_of_day: Read<'a, TimeOfDay>,
    positions: ReadStorage<'a, Pos>,
    velocities: ReadStorage<'a, Vel>,
    healths: ReadStorage<'a, Health>,
    energies: ReadStorage<'a, Energy>,
    bodies: ReadStorage<'a, Body>,
    players: ReadStorage<'a, Player>,
    char_states: ReadStorage<'a, CharacterState>,
}

impl<'a> System<'a> for Sys {
    type SystemData = ReadData<'a>;

    const NAME: &'static str = "telemetry";
    const ORIGIN: Origin = Origin::Common;
    const PHASE: Phase = Phase::Create;

    fn run(_job: &mut Job<Self>, data: Self::SystemData) {
        let tick = TICK_COUNTER.fetch_add(1, Ordering::Relaxed);
        if tick % SNAPSHOT_TICKS != 0 {
            return;
        }

        let tod = data.time_of_day.0;
        common::telemetry!("wc", tod = tod);

        for (pos, _vel, health, energy, player, char_state) in (
            &data.positions,
            &data.velocities,
            &data.healths,
            &data.energies,
            &data.players,
            &data.char_states,
        )
            .join()
        {
            let hp = health.current() as u32;
            let hp_max = health.maximum() as u32;
            let en = energy.current() as u32;
            let en_max = energy.maximum() as u32;
            let px = pos.0.x;
            let py = pos.0.y;
            let pz = pos.0.z;
            let state = format!("{:?}", char_state);
            let alias = &player.alias;
            common::telemetry!(
                "ps",
                player = alias,
                hp, hp_max, en, en_max,
                px, py, pz,
                state = state
            );

            // Entity context — entities near this player
            let mut nearby: Vec<(f32, String, u32, u32)> = Vec::new();
            for (other_pos, other_health, other_body) in
                (&data.positions, &data.healths, &data.bodies).join()
            {
                let dist_sq = (other_pos.0 - pos.0).magnitude_squared();
                if dist_sq < EC_RADIUS_SQ && dist_sq > 0.1 {
                    nearby.push((
                        dist_sq.sqrt(),
                        format!("{:?}", other_body),
                        other_health.current() as u32,
                        other_health.maximum() as u32,
                    ));
                }
            }
            nearby.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            nearby.truncate(EC_MAX_ENTITIES);

            let ec_str = nearby
                .iter()
                .map(|(d, k, h, hm)| format!("{d:.1}:{k}:{h}/{hm}"))
                .collect::<Vec<_>>()
                .join("|");

            common::telemetry!("ec", player = alias, entities = ec_str);
        }
    }
}
