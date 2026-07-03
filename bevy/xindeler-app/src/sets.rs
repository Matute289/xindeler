//! Canonical schedule / `SystemSet` layout (spec §2.2, EM-2.1).
//!
//! Every Xindeler system (client or server shell) is expected to live in one
//! of these sets so cross-crate ordering is declared in exactly one place:
//!
//! | Schedule      | Set                 | What goes there                              |
//! |---------------|---------------------|----------------------------------------------|
//! | `PreUpdate`   | [`NetSet`]          | network ingress (replicon receive), input    |
//! | `FixedUpdate` | [`SimSet`]          | embedded sim tick (server shell only)        |
//! | `Update`      | [`MirrorSet`]       | sim state -> Bevy entity mirroring (bridge)  |
//! | `Update`      | [`GameplaySet`]     | gameplay logic reading mirrored state        |
//! | `PostUpdate`  | [`PresentationSet`] | mesh-upload kickoff, audio, camera polish    |

use bevy::prelude::*;

/// `PreUpdate`: network ingress and raw input collection.
///
/// Replicon receive systems and anything translating OS/wire messages into
/// ECS state belong here, so everything downstream in the frame sees a
/// consistent snapshot.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NetSet;

/// `FixedUpdate`: the embedded specs simulation tick (server shell).
///
/// `Server::tick` (and future native sim systems) run here at the fixed sim
/// rate, decoupled from render framerate. The pure-Bevy client keeps this set
/// empty.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SimSet;

/// `Update`, before [`GameplaySet`]: sim -> Bevy mirroring.
///
/// Read-mostly bridge systems that copy sim state into (replicated) Bevy
/// entities. Runs before gameplay so gameplay always reads this frame's
/// mirrored state.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MirrorSet;

/// `Update`, after [`MirrorSet`]: per-frame gameplay/shell logic.
///
/// Camera controllers, UI-driven actions, client prediction — anything that
/// consumes mirrored state and produces intents/events for the sim.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GameplaySet;

/// `PostUpdate`: presentation kickoff.
///
/// Chunk-mesh upload budgeting, audio emission, atmosphere/fog application —
/// systems that turn final frame state into render/audio work.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PresentationSet;

/// Registers the canonical set layout on `app` (called by
/// [`crate::XindelerAppPlugin`]).
pub(crate) fn configure(app: &mut App) {
    app.configure_sets(PreUpdate, NetSet)
        .configure_sets(FixedUpdate, SimSet)
        .configure_sets(Update, (MirrorSet, GameplaySet).chain())
        .configure_sets(PostUpdate, PresentationSet);
}
