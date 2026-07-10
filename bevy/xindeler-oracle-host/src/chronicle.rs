//! BL-82 EM-4.8 — the `world_rumor → chronicle` narrative hook (task board
//! T47.10, spec §1.11).
//!
//! `DmEvent.narrative.world_rumor` (`crate::dm_event`) is meant to feed
//! BL-16's real chronicle/lore system — which does not exist yet. This
//! module is explicitly a STAND-IN seam, exactly like EM-4.2e's
//! `AiGatewayConfig`/EM-4.3's `DmEventLoader` before their real callers
//! existed: [`ChronicleLog`] is a plain in-memory, append-only,
//! bounded-length log the server exposes; [`chronicle_hook_system`] appends
//! every loaded `DmEvent`'s `world_rumor` (when present) to it, fully
//! automatically, the moment the asset finishes loading — no explicit
//! wiring call is needed (unlike [`crate::narrative`]'s `on_enter_message`
//! half, which genuinely needs a target `DimensionId` nothing produces yet).
//! BL-16 replaces this log with the real chronicle system later without
//! touching [`crate::dm_event::DmEvent`] or [`crate::dm_event::DmEventPlugin`].
use std::collections::VecDeque;

use bevy::{
    app::{App, Plugin, Update},
    asset::{AssetEvent, Assets},
    ecs::{
        message::MessageReader,
        resource::Resource,
        system::{Res, ResMut},
    },
};

use crate::dm_event::DmEvent;

/// Clamp bounds for [`ChronicleLog`] (anti-chaos, same posture as
/// `dm_event::bounds`): a misbehaving or malicious ORACLE event stream
/// dropping many `world_rumor`-carrying files must not grow this resource
/// unboundedly — oldest entries are dropped once the cap is hit.
pub mod bounds {
    /// Maximum number of entries [`super::ChronicleLog`] retains.
    pub const MAX_ENTRIES: usize = 1024;
}

/// In-memory, append-only chronicle-hook log (BL-82 EM-4.8). A BL-16 stand-in
/// — NOT the real chronicle/lore system, which doesn't exist yet. Bounded to
/// [`bounds::MAX_ENTRIES`] (oldest entries drop first) so this resource
/// cannot grow without limit for the lifetime of a long-running server.
#[derive(Resource, Debug, Clone, Default, PartialEq, Eq)]
pub struct ChronicleLog(VecDeque<String>);

impl ChronicleLog {
    /// Appends `text`, dropping the oldest entry first if already at
    /// [`bounds::MAX_ENTRIES`].
    pub fn push(&mut self, text: impl Into<String>) {
        if self.0.len() >= bounds::MAX_ENTRIES {
            self.0.pop_front();
        }
        self.0.push_back(text.into());
    }

    /// Iterates entries oldest-first.
    pub fn iter(&self) -> impl Iterator<Item = &str> { self.0.iter().map(String::as_str) }

    #[must_use]
    pub fn len(&self) -> usize { self.0.len() }

    #[must_use]
    pub fn is_empty(&self) -> bool { self.0.is_empty() }
}

/// Reacts to a `DmEvent` finishing (or re-finishing, on hot-reload) its load:
/// if [`DmEvent::narrative`]'s `world_rumor` is present, appends it to
/// [`ChronicleLog`] — within the same tick the `AssetEvent` is observed
/// (spec §1.11's "within one tick of the DmEvent loading" acceptance bar).
/// `AssetEvent::Removed`/`Unused` are ignored (nothing to append).
pub(crate) fn chronicle_hook_system(
    mut events: MessageReader<AssetEvent<DmEvent>>,
    assets: Res<Assets<DmEvent>>,
    mut log: ResMut<ChronicleLog>,
) {
    for event in events.read() {
        let id = match event {
            AssetEvent::Added { id } | AssetEvent::Modified { id } => *id,
            AssetEvent::Removed { .. } | AssetEvent::Unused { .. } | AssetEvent::LoadedWithDependencies { .. } => {
                continue;
            },
        };
        let Some(dm_event) = assets.get(id) else {
            continue;
        };
        if let Some(rumor) = &dm_event.narrative.world_rumor {
            log.push(rumor.clone());
        }
    }
}

/// Registers [`ChronicleLog`] + [`chronicle_hook_system`]. Requires
/// [`crate::dm_event::DmEventPlugin`] (so `Assets<DmEvent>`/
/// `AssetEvent<DmEvent>` exist) to already be present in the `App` — the
/// same ordering contract every consumer of a `bevy_asset` type has.
pub struct ChroniclePlugin;

impl Plugin for ChroniclePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ChronicleLog>()
            .add_systems(Update, chronicle_hook_system);
    }
}

#[cfg(test)]
mod tests {
    use bevy::{
        app::App,
        asset::{AssetPlugin, Assets},
        prelude::MinimalPlugins,
    };

    use super::*;
    use crate::dm_event::{DmEventPlugin, Narrative};

    fn new_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(AssetPlugin::default())
            .add_plugins(DmEventPlugin)
            .add_plugins(ChroniclePlugin);
        app.finish();
        app.update();
        app
    }

    /// A `DmEvent` with a `world_rumor`, added directly to `Assets<DmEvent>`
    /// (bypassing the file watcher — this test exercises the reacting SYSTEM,
    /// not the loader, which `dm_event.rs`'s own tests already cover), shows
    /// up in the chronicle log within a bounded, small number of updates —
    /// the literal EM-4.8 acceptance bar ("within one tick of the DmEvent
    /// loading").
    #[test]
    fn world_rumor_appears_in_the_chronicle_log() {
        let mut app = new_app();

        let event = DmEvent {
            narrative: Narrative {
                world_rumor: Some("A cold mist swallows the village.".to_owned()),
                on_enter_message: None,
            },
            ..Default::default()
        };
        // Retain the returned `Handle` for the test's lifetime — exactly
        // like a real consumer would (e.g. `dm_event.rs`'s own
        // `dropped_file_triggers_asset_added_within_1s` test keeps its
        // `AssetServer::load` handle alive the same way). Dropping the
        // handle immediately would make this the LAST strong reference,
        // and `Assets<T>` would unload the asset again before
        // `chronicle_hook_system` ever looks it up via `assets.get(id)`.
        let _handle = app
            .world_mut()
            .resource_mut::<Assets<DmEvent>>()
            .add(event);

        // Bevy's `AssetEvents` system flushes `Assets<T>`'s internal
        // "just added" queue into `Messages<AssetEvent<T>>` once per frame;
        // a small bounded number of updates tolerates that without the test
        // depending on exact intra-frame system ordering.
        let mut found = false;
        for _ in 0..5 {
            app.update();
            if app
                .world()
                .resource::<ChronicleLog>()
                .iter()
                .any(|entry| entry == "A cold mist swallows the village.")
            {
                found = true;
                break;
            }
        }
        assert!(
            found,
            "the world_rumor must appear in the chronicle log within a few ticks of the \
             DmEvent's AssetEvent::Added firing"
        );
    }

    /// A `DmEvent` with NO `world_rumor` never appends anything.
    #[test]
    fn absent_world_rumor_appends_nothing() {
        let mut app = new_app();

        app.world_mut()
            .resource_mut::<Assets<DmEvent>>()
            .add(DmEvent::default());

        for _ in 0..5 {
            app.update();
        }
        assert!(
            app.world().resource::<ChronicleLog>().is_empty(),
            "no world_rumor means no chronicle entry"
        );
    }

    /// [`ChronicleLog`] is bounded: pushing past `MAX_ENTRIES` drops the
    /// oldest entry first rather than growing without limit.
    #[test]
    fn chronicle_log_is_bounded() {
        let mut log = ChronicleLog::default();
        for i in 0..bounds::MAX_ENTRIES + 10 {
            log.push(format!("entry {i}"));
        }
        assert_eq!(log.len(), bounds::MAX_ENTRIES);
        // The oldest 10 entries ("entry 0".."entry 9") must have been
        // dropped; the log now starts at "entry 10".
        assert_eq!(log.iter().next(), Some("entry 10"));
    }
}


