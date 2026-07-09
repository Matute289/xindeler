//! EM-4.2 correctness: **persistence survives a full stop/restart of
//! `xindeler-server-app`**.
//!
//! `dual_stack.rs` (EM-4.1) proves a single session — connect, play, logout —
//! but never proves the persisted state is actually there on a SUBSEQUENT
//! boot of the SAME binary. This test does the full round trip:
//!
//!  1. Boot `xindeler-server-app` (process A) pointed at a fresh temp
//!     `VELOREN_USERDATA`.
//!  2. Connect a real client, create a character, then EDIT it (renames the
//!     `alias` — a real, cheaply observable persistent field distinct from the
//!     value picked at creation) and wait for the server's `CharacterEdited`
//!     acknowledgement, which server-side only fires after
//!     `execute_character_edit` has already run on the persistence thread
//!     (`server/src/persistence/character_updater.rs`) — i.e. by the time the
//!     client sees this event, the sqlite row is genuinely written.
//!  3. Log out cleanly, then send SIGTERM to process A (not `Child::kill()`,
//!     which is SIGKILL and would skip `Drop for Server` entirely) and wait for
//!     a clean exit — exercising the EM-4.1 graceful-shutdown path for real.
//!  4. Boot xindeler-server-app AGAIN (process B) pointed at the SAME data dir,
//!     connect a NEW client with the SAME username, and confirm the character
//!     list contains exactly the one character, with the EDITED alias — proving
//!     the state survived a genuine process stop/restart, not just "the file
//!     exists".
//!
//! Also asserts `sim.rs`'s documented claim that `xindeler-server-app` uses
//! the SAME `<userdata>/server/saves/db.sqlite` sqlite path server-cli uses
//! (`server/src/persistence/mod.rs`'s `establish_connection`), by checking
//! the file materializes at exactly that path.
//!
//! Needs real assets (`VELOREN_ASSETS`/`XINDELER_ASSETS` + the LFS map
//! blobs) and spawns two real child processes in sequence, so — like
//! `dual_stack.rs` — this is `#[ignore]`d; run locally with:
//! `VELOREN_ASSETS="$(pwd)/assets" cargo test -p xindeler-server-app --test
//! persistence_roundtrip -- --ignored --nocapture`

mod support;

use std::{
    net::SocketAddr,
    time::{Duration, Instant},
};

use client::Event as ClientEvent;
use common::{character::CharacterId, clock::Clock};
use support::{
    TPS, client_runtime, connect_client, create_character, first_character_id, graceful_shutdown,
    logout, seed_settings, spawn_server_app, wait_for, wait_for_listener,
};

const USERNAME: &str = "em42_persistence_bot";
const ORIGINAL_ALIAS: &str = "PersistOriginal";
const EDITED_ALIAS: &str = "PersistRenamed";

#[test]
#[ignore = "spawns two real xindeler-server-app processes in sequence + boots a real world: needs \
            assets + LFS; run locally with VELOREN_ASSETS"]
fn persistence_survives_full_restart() {
    let data_dir = tempfile::tempdir().expect("tempdir");
    let game_port = seed_settings(data_dir.path());
    let addr: SocketAddr = ([127, 0, 0, 1], game_port).into();

    // ---- process A: create + edit a character, then shut down cleanly ----
    let mut child_a = spawn_server_app(data_dir.path(), "server-app-A");
    wait_for_listener(addr, Duration::from_secs(60), "A");

    let runtime = client_runtime("tokio-em42-persist-client");
    let connect_deadline = Instant::now() + Duration::from_secs(60);
    let mut client = connect_client(
        &runtime,
        &mut child_a,
        game_port,
        USERNAME,
        connect_deadline,
    );
    println!("[test] connected to process A");

    let mut clock = Clock::new(Duration::from_secs_f64(1.0 / TPS));
    let deadline = Instant::now() + Duration::from_secs(60);

    client.load_character_list();
    wait_for(
        &mut client,
        &mut clock,
        deadline,
        "character-list",
        &mut child_a,
        |c, _| (!c.character_list().loading).then_some(()),
    );
    assert!(
        first_character_id(&client).is_none(),
        "fresh data dir should start with no characters"
    );

    let character_id = create_character(
        &mut client,
        &mut clock,
        deadline,
        &mut child_a,
        ORIGINAL_ALIAS,
    );
    println!("[test] character created: id={character_id} alias={ORIGINAL_ALIAS}");

    // Edit the alias — a real, server-persisted mutation distinct from the
    // value chosen at creation (proves this is an actual UPDATE round trip,
    // not just "creation happened to be visible").
    client.edit_character(
        EDITED_ALIAS.to_owned(),
        CharacterId(character_id),
        support::default_body().into(),
    );
    wait_for(
        &mut client,
        &mut clock,
        deadline,
        "edit-character",
        &mut child_a,
        |_, events| {
            events.iter().find_map(|e| match e {
                ClientEvent::CharacterEdited(id) if id.0 == character_id => Some(()),
                _ => None,
            })
        },
    );
    println!("[test] character edited: id={character_id} alias={EDITED_ALIAS}");

    logout(&mut client, &mut clock);
    drop(client);
    println!("[test] logged out of process A; sending SIGTERM");

    graceful_shutdown(&mut child_a, Duration::from_secs(30), "A");
    println!("[test] process A exited cleanly (graceful SIGTERM shutdown confirmed)");

    // ---- confirm the sqlite path `sim.rs` claims is real ----
    let db_path = data_dir
        .path()
        .join("server")
        .join("saves")
        .join("db.sqlite");
    let db_meta = std::fs::metadata(&db_path).unwrap_or_else(|e| {
        panic!(
            "expected sqlite database at {} (the path `sim.rs`'s doc comment claims matches \
             server-cli's `<userdata>/server`) but it's missing: {e:?}",
            db_path.display()
        )
    });
    assert!(
        db_meta.len() > 0,
        "sqlite database at {} exists but is empty",
        db_path.display()
    );
    println!(
        "[test] confirmed sqlite database at {} ({} bytes) — matches sim.rs's documented \
         `<userdata>/server` data-dir claim",
        db_path.display(),
        db_meta.len()
    );

    // ---- process B: same data dir, fresh boot, verify the edit survived ----
    let mut child_b = spawn_server_app(data_dir.path(), "server-app-B");
    wait_for_listener(addr, Duration::from_secs(60), "B");

    let connect_deadline = Instant::now() + Duration::from_secs(60);
    let mut client = connect_client(
        &runtime,
        &mut child_b,
        game_port,
        USERNAME,
        connect_deadline,
    );
    println!("[test] connected to process B (same data dir as process A)");

    let mut clock = Clock::new(Duration::from_secs_f64(1.0 / TPS));
    let deadline = Instant::now() + Duration::from_secs(60);

    client.load_character_list();
    wait_for(
        &mut client,
        &mut clock,
        deadline,
        "character-list-reload",
        &mut child_b,
        |c, _| (!c.character_list().loading).then_some(()),
    );

    let characters = &client.character_list().characters;
    assert_eq!(
        characters.len(),
        1,
        "expected exactly the one character created in process A to survive the restart, got: \
         {characters:?}"
    );
    let reloaded = &characters[0].character;
    assert_eq!(
        reloaded.id,
        Some(CharacterId(character_id)),
        "reloaded character id should match the one created in process A"
    );
    assert_eq!(
        reloaded.alias, EDITED_ALIAS,
        "reloaded character alias should be the EDITED value ({EDITED_ALIAS}), proving the edit — \
         not just the original creation — survived the stop/restart round trip"
    );
    println!(
        "[test] persistence round trip confirmed: character {} (\"{}\") survived a full SIGTERM \
         stop + restart of xindeler-server-app",
        character_id, reloaded.alias
    );

    logout(&mut client, &mut clock);
    drop(client);
}
