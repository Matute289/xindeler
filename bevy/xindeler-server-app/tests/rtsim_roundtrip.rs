//! EM-4.2 correctness: **rtsim genuinely ticks and saves under the Bevy
//! shell — it isn't idling, and its save-on-shutdown path really runs.**
//!
//! rtsim (the long-running world simulation — NPC migrations, factions,
//! economy) is driven by `rtsim::tick::Sys`, a normal specs system
//! registered into the SAME dispatcher every other server system runs on
//! (`server::rtsim::add_server_systems`, gated on the `worldgen` feature,
//! which `xindeler-server-app`'s `server` dependency has ON by default —
//! see `Cargo.toml`'s comment). Concretely, `RtState::tick`
//! (`rtsim/src/lib.rs`) increments `Data::tick` by exactly 1 every time it
//! runs, so `Data::tick` is a 1:1 proxy for "how many server ticks has rtsim
//! actually been driven for" — a stronger, non-inferential signal than
//! grepping log lines.
//!
//! This test does NOT need a connected client — rtsim advances independent
//! of any player (this is the whole point of a persistent world
//! simulation), so it just boots the server and watches the persisted
//! `<userdata>/server/rtsim/data.dat` file.
//!
//! Two things are verified:
//!  1. **Advancing, not idling**: after letting the server run past rtsim's own
//!     60s periodic-autosave interval (`server/src/rtsim/mod.rs`'s
//!     `Duration::from_secs(60)` check in `rtsim::tick::Sys::run`), `tick` is
//!     well past zero and keeps climbing between two snapshots taken a further
//!     ~15s apart.
//!  2. **Save-on-shutdown really runs and really loads back**: SIGTERM triggers
//!     `Drop for Server`'s `rtsim.save(true)` (blocking — see `shutdown.rs`'s
//!     doc comment, "Waiting for rtsim save thread to finish..."); a SECOND
//!     boot of the SAME binary against the SAME data dir resumes from a `tick`
//!     value that continues climbing from where the first process left off,
//!     rather than resetting near zero — which is the only way to distinguish
//!     "genuinely reloaded the saved sim state" from "silently regenerated a
//!     fresh world".
//!
//! Needs real assets + LFS (worldgen is required even though we never
//! connect a client — rtsim's own `Data::generate` needs a real `World` +
//! `IndexRef`), so — like the other full-world tests in this crate — this
//! is `#[ignore]`d; run locally with:
//! `VELOREN_ASSETS="$(pwd)/assets" cargo test -p xindeler-server-app --test
//! rtsim_roundtrip -- --ignored --nocapture`

mod support;

use std::{
    net::SocketAddr,
    time::{Duration, Instant},
};

use rtsim::data::Data;
use support::{graceful_shutdown, seed_settings, spawn_server_app, wait_for_listener};

/// How long to let rtsim run past its own 60s autosave interval before the
/// first snapshot, so the snapshot is guaranteed to reflect at least one
/// completed periodic save (not just the final shutdown save).
const PHASE_1_RUN: Duration = Duration::from_secs(70);
/// Extra run time between the two snapshots within the same process, so
/// `tick` has room to visibly climb between them.
const PHASE_2_RUN: Duration = Duration::from_secs(15);
/// Short run after the restart — just long enough to prove the resumed
/// counter keeps growing from a large base rather than restarting near zero.
const PHASE_3_RUN: Duration = Duration::from_secs(10);

fn rtsim_data_path(data_dir: &std::path::Path) -> std::path::PathBuf {
    data_dir.join("server").join("rtsim").join("data.dat")
}

fn read_rtsim_data(path: &std::path::Path) -> Data {
    let file = std::fs::File::open(path)
        .unwrap_or_else(|e| panic!("expected rtsim save file at {}: {e:?}", path.display()));
    *Data::from_reader(std::io::BufReader::new(file)).unwrap_or_else(|e| {
        panic!(
            "rtsim save file at {} failed to decode via the production `Data::from_reader` path: \
             {e:?}",
            path.display()
        )
    })
}

#[test]
#[ignore = "spawns two real xindeler-server-app processes in sequence + boots a real world: needs \
            assets + LFS; run locally with VELOREN_ASSETS. Runs for ~95s (letting rtsim cross its \
            own 60s autosave interval) + a short restart phase."]
fn rtsim_ticks_and_survives_restart() {
    let data_dir = tempfile::tempdir().expect("tempdir");
    let game_port = seed_settings(data_dir.path());
    let addr: SocketAddr = ([127, 0, 0, 1], game_port).into();
    let data_path = rtsim_data_path(data_dir.path());

    // ---- process A: let rtsim run past one periodic autosave ----
    let mut child_a = spawn_server_app(data_dir.path(), "server-app-A");
    let boot_start = Instant::now();
    wait_for_listener(addr, Duration::from_secs(60), "A");
    println!(
        "[test] process A listening after {:?}; letting rtsim run for {PHASE_1_RUN:?} (past its \
         own 60s periodic-autosave interval)",
        boot_start.elapsed()
    );

    std::thread::sleep(PHASE_1_RUN);
    let snapshot_1 = read_rtsim_data(&data_path);
    println!(
        "[test] snapshot 1 (after periodic autosave): tick={} time_of_day={:.2}",
        snapshot_1.tick, snapshot_1.time_of_day.0
    );
    assert!(
        snapshot_1.tick > 500,
        "expected rtsim to have ticked well past zero after {PHASE_1_RUN:?} of real run time \
         (>500 ticks at the ~30Hz server rate), got tick={} — rtsim looks idle, not driven by the \
         shell's per-frame Server::tick()",
        snapshot_1.tick
    );

    std::thread::sleep(PHASE_2_RUN);

    // SIGTERM triggers `Drop for Server`'s blocking `rtsim.save(true)` — the
    // final save this assertion needs to observe.
    graceful_shutdown(&mut child_a, Duration::from_secs(30), "A");
    println!("[test] process A exited cleanly (graceful SIGTERM shutdown confirmed)");

    let snapshot_2 = read_rtsim_data(&data_path);
    println!(
        "[test] snapshot 2 (after final shutdown save): tick={} time_of_day={:.2}",
        snapshot_2.tick, snapshot_2.time_of_day.0
    );
    assert!(
        snapshot_2.tick > snapshot_1.tick,
        "expected rtsim's tick counter to have advanced further between the periodic autosave \
         (tick={}) and the final SIGTERM-triggered save (tick={}) — if these are equal, either \
         rtsim stopped ticking or the shutdown save didn't actually capture a fresh state",
        snapshot_1.tick,
        snapshot_2.tick
    );
    println!(
        "[test] confirmed rtsim advanced {} further ticks between the periodic and final saves — \
         the shutdown save is capturing live, current state, not a stale snapshot",
        snapshot_2.tick - snapshot_1.tick
    );

    // ---- process B: same data dir, confirm rtsim RESUMES rather than resets ----
    let mut child_b = spawn_server_app(data_dir.path(), "server-app-B");
    wait_for_listener(addr, Duration::from_secs(60), "B");
    std::thread::sleep(PHASE_3_RUN);
    graceful_shutdown(&mut child_b, Duration::from_secs(30), "B");
    println!("[test] process B exited cleanly (graceful SIGTERM shutdown confirmed)");

    let snapshot_3 = read_rtsim_data(&data_path);
    println!(
        "[test] snapshot 3 (after restart + {PHASE_3_RUN:?}): tick={} time_of_day={:.2}",
        snapshot_3.tick, snapshot_3.time_of_day.0
    );
    assert!(
        snapshot_3.tick > snapshot_2.tick,
        "expected rtsim's tick counter after restarting xindeler-server-app against the SAME data \
         dir to RESUME climbing from snapshot 2's tick={} (proving `RtSim::new` really loaded the \
         saved data.dat via `Data::from_reader`), got tick={} — a value at or near zero would \
         mean the save was silently ignored and a fresh world was regenerated instead of the \
         persisted one being loaded",
        snapshot_2.tick,
        snapshot_3.tick
    );
    println!(
        "[test] rtsim save/reload round trip confirmed: tick counter resumed from {} and reached \
         {} after restarting the SAME xindeler-server-app binary against the SAME data dir — the \
         persisted rtsim state was genuinely reloaded, not regenerated",
        snapshot_2.tick, snapshot_3.tick
    );
}
