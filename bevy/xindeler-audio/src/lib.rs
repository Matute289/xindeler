//! `xindeler-audio` — BL-82 EM-5.10a (T56.34): the Kira-on-`cpal` audio
//! foundation for the Bevy client.
//!
//! ## Locked architecture decision (spec §Q2=B)
//! Kira is integrated **directly** in this crate — NOT via the
//! `bevy_kira_audio` wrapper crate. We own all the Bevy-integration glue (the
//! `AssetLoader`, resource wiring, volume application, diagnostics) ourselves,
//! mirroring the pre-migration client's own architecture at
//! `voxygen/src/audio/` (`mod.rs`, `channel.rs`) — ported into idiomatic Bevy
//! (a [`Plugin`] + [`Resource`]s + systems) rather than copied verbatim (that
//! code predates Bevy and is structured around a bespoke non-ECS frontend).
//!
//! ## Scope of THIS crate (T56.34 / 5.10a only)
//! Just the backend core: [`AudioManager`](kira::AudioManager) init on the
//! default `cpal` output device, 4 named mixer sub-tracks (music/ui/sfx/
//! ambience) + master, a [`XindelerAudioAsset`] loader for `.ogg`/`.wav`,
//! per-track/master volume, and a low-rate diagnostics pump. SFX triggers,
//! music/ambience state machines, spatial audio, and the instrument bank are
//! later phases (5.10b–e, T56.35–.37) — nothing here is wired to gameplay yet.
//!
//! ## Kira 0.12.2 API notes (verified against the actual crate source + a
//! real playback probe on this dev machine, not assumed from an older pin)
//! - `voxygen/Cargo.toml`'s pin (`kira = "0.12", features = ["cpal",
//!   "symphonia", "ogg", "vorbis"]`) matches the latest 0.12.x on crates.io
//!   (0.12.2) — no version drift to reconcile.
//! - **`AudioManager<CpalBackend>` is `Send + Sync`** — empirically verified
//!   (compile-time `assert_send`/`assert_sync` probe) on this dev machine
//!   (macOS/CoreAudio) and confirmed by reading `kira`'s `CpalBackend` source:
//!   the actual `!Send`-prone platform stream (`cpal::Stream`) is moved onto a
//!   dedicated audio thread inside `CpalBackend::start` (`StreamManager`); only
//!   `Arc<Atomic*>`/`Mutex`-guarded handles cross back to the caller-facing
//!   `CpalBackend` struct. Cross-checked the Linux path too (`cpal`'s ALSA
//!   `Device` holds only `String`/`Option<String>`/an enum/`Arc<AlsaContext>` —
//!   no raw pointers), so this should hold on the `ubuntu-latest` CI runner as
//!   well. Consequently [`AudioBackend`] is a plain [`Resource`], not `NonSend`
//!   — simpler, and correct per the actual bounds rather than an assumed worst
//!   case.
//! - **No per-frame "pump" is needed for `CpalBackend` playback correctness.**
//!   `Renderer::on_start_processing`/`process` — the calls that actually
//!   advance tweens, mix samples, and prune finished sounds — are invoked by
//!   the backend's own dedicated audio-callback thread and are not even
//!   reachable from `AudioManager`'s public API for this backend. Contrast
//!   `kira::backend::mock::MockBackend` (its test/benchmark backend), which
//!   *does* require the caller to invoke those two methods manually — the two
//!   backends are deliberately asymmetric. Confirmed empirically too: a scratch
//!   play-a-sine-wave-then-sleep probe transitions `PlaybackState::Playing ->
//!   Stopped` and self-prunes (`track.num_sounds()` `1 -> 0`) with zero
//!   host-side calls in between. [`diagnostics::pump_audio_diagnostics`]
//!   therefore is NOT "servicing the engine" (there is nothing to service) — it
//!   is a low-rate diagnostics drain (`CpalBackend::pop_error`), documented in
//!   full on that function.
//! - Handle/tween lifetime: dropping a `TrackHandle`/sound handle does not stop
//!   playback (Kira sounds are fire-and-forget once played), but a *finished*
//!   sound's backing resource is pruned automatically by the mixer's own
//!   `remove_and_add(|sound| sound.finished())` pass on every audio-thread
//!   processing cycle (`kira::track::sub`) — no extra bookkeeping resource is
//!   needed on our side for T56.34's scope (no SFX/music channel pooling exists
//!   yet; that machinery is 5.10b/c's job and will decide there whether it
//!   needs to retain handles for control, same as `voxygen`'s own
//!   `channel.rs`).
//! - `.wav` decoding needs `symphonia/pcm` in addition to `symphonia/wav` — the
//!   demuxer alone fails ("unsupported audio codec") on a real WAV fixture; see
//!   `Cargo.toml`'s dependency comment and [`asset`]'s doc comment for the
//!   empirical check.

pub mod ambience;
mod asset;
mod diagnostics;
mod manager;
pub mod music;
pub mod sfx;
mod volume;

pub use asset::{XindelerAudioAsset, XindelerAudioAssetLoader};
pub use manager::{AudioBackend, AudioTracks};
pub use volume::AudioVolumes;

use bevy::prelude::*;
use kira::Decibels;

/// Adds the Kira-backed audio foundation to the app: manager + 4 mixer
/// sub-tracks + master, the `.ogg`/`.wav` asset pipeline, and volume/
/// diagnostics upkeep systems.
///
/// Resilient by design (matches the codebase's "log and continue, don't crash
/// over an optional subsystem" convention, e.g. `listen_server.rs`'s
/// embedded-world-boot-failure handling): if no `cpal` output device is
/// available (headless CI runners commonly have none), [`AudioBackend`]
/// becomes [`AudioBackend::Unavailable`] and every system in this crate
/// becomes a cheap no-op — the app boots normally either way.
#[derive(Default)]
pub struct XindelerAudioPlugin;

impl Plugin for XindelerAudioPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<XindelerAudioAsset>()
            .init_asset_loader::<XindelerAudioAssetLoader>()
            .init_resource::<AudioVolumes>();

        manager::insert_audio_backend(app);

        app.add_systems(
            Update,
            (
                // Change-detected, not an unconditional per-tick write (this
                // codebase's established perf convention) — only runs the
                // (cheap, but non-zero: 5 command-writer pushes) volume push
                // when `AudioVolumes` actually changed.
                volume::apply_audio_volumes.run_if(resource_changed::<AudioVolumes>),
                diagnostics::pump_audio_diagnostics,
            ),
        );

        // BL-82 EM-5.10b (T56.35): the sfx.ron manifest + playback asset
        // cache. Folded into THIS plugin (extend, don't fork) rather than a
        // second plugin callers would need to remember to add — every
        // consumer of `XindelerAudioPlugin` gets SFX-ready-to-trigger for
        // free; nothing plays a sound until `xindeler-client::sfx`'s
        // event-mapper systems are ALSO added (a separate plugin, since they
        // need `xindeler-protocol` types this crate does not depend on).
        app.add_plugins(sfx::SfxManifestPlugin);
        // BL-82 EM-5.10c (T56.36): the soundtrack.ron/music_transition_
        // manifest.ron + ambience.ron manifests + their playback state, same
        // "folded into THIS plugin" reasoning as sfx above — the actual
        // per-frame state machine/orchestrator (reading real mirrored
        // hostile/weather/day-period state) is
        // `xindeler-client::music`/`xindeler-client::ambience`, added
        // separately.
        app.add_plugins((music::MusicManifestPlugin, ambience::AmbienceManifestPlugin));
    }
}

/// Converts a linear amplitude (`0.0..=1.0`, occasionally boosted past `1.0`
/// for a quiet source) to the [`Decibels`] Kira's volume APIs take.
///
/// Ported from `voxygen/src/audio/mod.rs::to_decibels` verbatim (same
/// special-cased silence/identity shortcuts) — Kira 0.12 only provides the
/// inverse conversion (`Decibels::as_amplitude`), not this direction.
pub(crate) fn to_decibels(amplitude: f32) -> Decibels {
    if amplitude <= 0.001 {
        Decibels::SILENCE
    } else if amplitude == 1.0 {
        Decibels::IDENTITY
    } else {
        Decibels(amplitude.log10() * 20.0)
    }
}

#[cfg(test)]
mod tests {
    use std::{io::Write, time::Duration};

    use bevy::asset::AssetPlugin;
    use kira::{
        Frame,
        sound::{PlaybackState, static_sound::StaticSoundData},
        track::TrackHandle,
    };

    use super::*;

    /// Builds a tiny in-memory sine wave directly as a [`StaticSoundData`] —
    /// simpler than shipping a binary fixture through the asset pipeline for
    /// a test that only cares about the manager/tracks/volume plumbing (the
    /// `AssetLoader` itself gets its own dedicated test in `asset.rs`).
    fn sine_wave(duration_secs: f32, freq: f32) -> StaticSoundData {
        let sample_rate = 44_100u32;
        let n = (sample_rate as f32 * duration_secs) as usize;
        let frames: Vec<Frame> = (0..n)
            .map(|i| {
                let t = i as f32 / sample_rate as f32;
                let s = (std::f32::consts::TAU * freq * t).sin() * 0.2;
                Frame::new(s, s)
            })
            .collect();
        StaticSoundData {
            sample_rate,
            frames: frames.into(),
            settings: Default::default(),
            slice: None,
        }
    }

    fn boot_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(AssetPlugin::default());
        app.add_plugins(XindelerAudioPlugin);
        app.finish();
        app.update();
        app
    }

    #[test]
    fn plugin_boots_without_panicking_and_reports_availability() {
        let app = boot_app();
        let backend = app.world().resource::<AudioBackend>();
        // Whichever branch this environment lands in, the resource must be a
        // clean, well-formed enum state — never a partially-built manager
        // (e.g. `Ready` with a missing sub-track).
        match backend {
            AudioBackend::Ready { tracks, .. } => {
                assert_eq!(tracks.music.num_sounds(), 0);
                assert_eq!(tracks.ui.num_sounds(), 0);
                assert_eq!(tracks.sfx.num_sounds(), 0);
                assert_eq!(tracks.ambience.num_sounds(), 0);
            },
            AudioBackend::Unavailable => {
                // No real cpal output device in this environment (e.g. a
                // headless CI runner) — the resilient no-op path. Not a
                // failure: see this module's doc comment.
            },
        }
        // `AudioVolumes` defaults must exist unconditionally (settings UI
        // reads this in EM-5.12 regardless of whether real audio hardware
        // exists).
        let volumes = app.world().resource::<AudioVolumes>();
        assert!(volumes.master > 0.0);
    }

    /// The task's core verification: play a real one-shot tone through a
    /// SPECIFIC track (`sfx`) at a SET volume and observe real playback
    /// state transitions — not merely "compiles and doesn't panic".
    ///
    /// Runs the full real-audio assertion only when a genuine `cpal` device
    /// initialized (true on this dev machine, verified empirically before
    /// writing this test); on a device-less CI runner it degrades to the same
    /// documented no-op skip as the test above, rather than faking success.
    #[test]
    fn plays_a_one_shot_tone_through_a_chosen_track_at_a_set_volume() {
        let mut app = boot_app();
        let mut backend = app.world_mut().resource_mut::<AudioBackend>();
        let AudioBackend::Ready { tracks, .. } = &mut *backend else {
            eprintln!(
                "skipping real-playback assertions: no cpal output device in this environment"
            );
            return;
        };

        // A concrete "set volume": -6 dB baked into the sound instance
        // itself (Kira's own per-sound `.volume()`, independent of the
        // track's mixer volume this crate's `apply_audio_volumes` controls).
        let tone = sine_wave(0.05, 440.0).volume(-6.0);

        let sfx: &mut TrackHandle = &mut tracks.sfx;
        assert_eq!(sfx.num_sounds(), 0, "sfx track must start empty");
        let handle = sfx.play(tone).expect("sfx track has room to play");
        assert_eq!(handle.state(), PlaybackState::Playing);
        assert_eq!(sfx.num_sounds(), 1);

        // Real-time playback on a real device: wait past the clip's
        // duration, then observe Kira's OWN audio-thread GC (see this
        // crate's root doc comment) prune it without any pump call from us.
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(handle.state(), PlaybackState::Stopped);
        assert_eq!(sfx.num_sounds(), 0, "finished sound must self-prune");
    }

    #[test]
    fn changing_audio_volumes_applies_without_panicking() {
        let mut app = boot_app();
        app.insert_resource(AudioVolumes {
            master: 0.4,
            music: 0.1,
            ui: 0.9,
            sfx: 0.0,
            ambience: 1.0,
        });
        // `apply_audio_volumes` is change-detected (`resource_changed`); this
        // update is the one that must observe the change and push it to
        // Kira's command writers without panicking, regardless of whether a
        // real device backs the manager.
        app.update();
        // A second update with no further change must be a true no-op path
        // (exercises the `run_if` gate itself, not just the system body).
        app.update();
    }

    /// Real end-to-end `.wav` decode through the `AssetLoader`, not just the
    /// in-memory `StaticSoundData` path the tests above use. Hand-builds a
    /// minimal PCM16 mono WAV (RIFF/WAVE/fmt /data chunks) rather than adding
    /// a WAV-writer dev-dependency for one fixture.
    #[test]
    fn loads_a_real_wav_file_through_the_asset_loader() {
        let dir = tempfile::tempdir().expect("tempdir");
        let wav_path = dir.path().join("tone.wav");
        {
            let mut file = std::fs::File::create(&wav_path).expect("create wav fixture");
            file.write_all(&build_test_wav())
                .expect("write wav fixture");
        }

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(AssetPlugin {
            file_path: dir.path().to_string_lossy().into_owned(),
            ..Default::default()
        });
        app.add_plugins(XindelerAudioPlugin);
        app.finish();
        app.update();

        let asset_server = app.world().resource::<AssetServer>().clone();
        let handle: Handle<XindelerAudioAsset> = asset_server.load("tone.wav");

        let mut loaded = None;
        for _ in 0..200 {
            app.update();
            let assets = app.world().resource::<Assets<XindelerAudioAsset>>();
            if let Some(asset) = assets.get(&handle) {
                loaded = Some(asset.0.clone());
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }

        let sound = loaded.expect(".wav fixture did not load within ~1s");
        assert!(!sound.frames.is_empty(), "decoded WAV must contain samples");
        assert_eq!(sound.sample_rate, 8_000);
    }

    /// A minimal single-channel, 16-bit PCM, 8 kHz WAV containing 100
    /// samples of a synthesized tone — hand-assembled RIFF/WAVE bytes (no
    /// external WAV-writer crate needed for one small fixture).
    fn build_test_wav() -> Vec<u8> {
        let sample_rate: u32 = 8_000;
        let bits_per_sample: u16 = 16;
        let num_channels: u16 = 1;
        let samples: Vec<i16> = (0..100)
            .map(|i| {
                let t = i as f32 / sample_rate as f32;
                ((t * 440.0 * std::f32::consts::TAU).sin() * 8_000.0) as i16
            })
            .collect();

        let data_bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let byte_rate = sample_rate * u32::from(num_channels) * u32::from(bits_per_sample) / 8;
        let block_align = num_channels * bits_per_sample / 8;

        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_bytes.len() as u32).to_le_bytes());
        wav.extend_from_slice(b"WAVE");
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
        wav.extend_from_slice(&1u16.to_le_bytes()); // PCM format tag
        wav.extend_from_slice(&num_channels.to_le_bytes());
        wav.extend_from_slice(&sample_rate.to_le_bytes());
        wav.extend_from_slice(&byte_rate.to_le_bytes());
        wav.extend_from_slice(&block_align.to_le_bytes());
        wav.extend_from_slice(&bits_per_sample.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(data_bytes.len() as u32).to_le_bytes());
        wav.extend_from_slice(&data_bytes);
        wav
    }
}
