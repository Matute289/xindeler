//! [`AuroraOverlay`] — AURORA's (BL-15/BL-83) per-NPC extension state (BL-82
//! EM-4.2f, spec §1.5). This module defines the FULL final wire shape now
//! (short-term memory, intention vector, discrete emotional state) so BL-83
//! reads/writes against a settled schema rather than a placeholder that gets
//! redesigned later — but this task fills it with nothing but fixed neutral
//! constants; it computes no real memory, intention, or emotion from any NPC
//! position/stats/behavior history. That inference is BL-83's job.
//!
//! ## The Offline-fallback invariant (read this before touching either side)
//! [`AuroraOverlay`] stays completely empty (zero entries) while
//! [`AiExecutionMode::Offline`](crate::AiExecutionMode) is active — **Offline
//! means the map is empty, and an empty map means the NPC behaves exactly as
//! today's default `server-agent` AI (stalk/aggro/flee presets). This is not a
//! degraded mode — it is the current game, unchanged.** The moment the mode
//! becomes `LocalOnly` or `Full`, every mirrored NPC gets exactly ONE
//! [`AuroraNpcState`] entry, but every field in that entry is a
//! [`AuroraNpcState::default`] neutral placeholder (empty memory, `Idle`-only
//! intention, neutral mood at zero intensity) until BL-83 lands real
//! AURORA-authored content. A future reader of this resource must NEVER
//! assume a populated entry carries real data — only that it exists with
//! legible neutral defaults outside `Offline`.
//!
//! ## Why the schema is closed enums, not open strings/raw floats
//! [`IntentKind`]/[`MoodKind`] are small closed `Serialize`/`Deserialize`
//! enums, not a raw string or an embedding-shaped float vector — this keeps
//! the wire payload bounded and anti-chaos-clampable (an unrecognized variant
//! simply fails to deserialize rather than admitting an unbounded value),
//! matching every other RON-ingested schema in Phase 4
//! (`AtmosphereProfile`, `AiGatewayConfig`).

use std::collections::{HashMap, VecDeque};

use bevy::ecs::resource::Resource;
use serde::{Deserialize, Serialize};

/// A bounded ring buffer of recent event-summary strings (BL-82 EM-4.2f).
///
/// This is NOT raw dialogue/LLM output — this task writes no real summaries,
/// only the container + its capacity. BL-83 pushes real summaries later via
/// [`Self::push`], which evicts the oldest entry once [`Self::CAPACITY`] is
/// reached (ring-buffer semantics), so a per-NPC memory can never grow
/// unbounded.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShortTermMemory(VecDeque<String>);

impl ShortTermMemory {
    /// Maximum number of remembered summaries. Chosen by spec §1.5 (worksheet
    /// [Q2]) as a small, cheap-to-replicate bound.
    pub const CAPACITY: usize = 8;

    /// Appends `summary`, evicting the oldest entry first if already at
    /// [`Self::CAPACITY`].
    pub fn push(&mut self, summary: String) {
        if self.0.len() >= Self::CAPACITY {
            self.0.pop_front();
        }
        self.0.push_back(summary);
    }

    /// Iterates oldest-to-newest.
    pub fn iter(&self) -> impl Iterator<Item = &String> { self.0.iter() }

    #[must_use]
    pub fn is_empty(&self) -> bool { self.0.is_empty() }

    #[must_use]
    pub fn len(&self) -> usize { self.0.len() }
}

/// A small closed set of legible intent categories (BL-82 EM-4.2f, spec
/// §1.5) — NOT a raw float embedding. BL-83 fills real per-NPC weights later;
/// this task ships only the shape and the `Idle`-only neutral default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum IntentKind {
    /// No active intent — the neutral default.
    #[default]
    Idle,
    /// Following a patrol route.
    Patrol,
    /// Fleeing a threat.
    Flee,
    /// Actively seeking something (a target, an item, a destination).
    Seek,
    /// Seeking social interaction with another NPC/the player.
    Socialize,
    /// Guarding a position or ward.
    Guard,
}

/// A small closed set of discrete moods (BL-82 EM-4.2f, spec §1.5) — NOT a
/// raw valence/arousal float pair, so the wire payload stays legible for a
/// future HUD/debug display and cheap to replicate. BL-83 fills real
/// per-NPC moods later; this task ships only the shape and the `Neutral`
/// default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum MoodKind {
    /// No particular mood — the neutral default.
    #[default]
    Neutral,
    /// Calm/at-ease.
    Content,
    /// On alert, uneasy.
    Wary,
    /// Frightened.
    Afraid,
    /// Hostile/aggressive.
    Hostile,
    /// Interested/inquisitive.
    Curious,
}

/// Discrete mood + intensity (BL-82 EM-4.2f, spec §1.5). `intensity` is a
/// plain magnitude in `[0.0, 1.0]` for HOW strongly `mood` applies — this
/// task never computes it; it is always `0.0` (neutral) until BL-83 writes
/// real data.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EmotionalState {
    pub mood: MoodKind,
    pub intensity: f32,
}

impl Default for EmotionalState {
    fn default() -> Self {
        Self {
            mood: MoodKind::default(),
            intensity: 0.0,
        }
    }
}

/// Per-NPC AURORA extension state (BL-82 EM-4.2f, spec §1.5's full final wire
/// schema). [`Self::default`] IS the documented neutral placeholder this task
/// populates every entry with: empty memory, `Idle`-only intention at weight
/// `1.0`, neutral mood at zero intensity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuroraNpcState {
    /// Recent event-summary strings, oldest-to-newest, capped at
    /// [`ShortTermMemory::CAPACITY`]. Empty until BL-83 writes real summaries.
    pub short_term_memory: ShortTermMemory,
    /// Sparse weighted intent vector (spec §1.5 pins this exact type) — an
    /// absent [`IntentKind`] is implicitly weight `0.0`, so the neutral
    /// default is the single entry `[(Idle, 1.0)]` rather than one entry per
    /// variant. This `Vec` is not compiler-bounded the way
    /// [`ShortTermMemory`] is; in practice it stays small because
    /// [`IntentKind`] itself is a small closed enum — BL-83 should write at
    /// most one weight per distinct kind, not treat this as an open list.
    pub intention: Vec<(IntentKind, f32)>,
    /// Discrete mood + intensity. Neutral default is `(Neutral, 0.0)`.
    pub emotional_state: EmotionalState,
}

impl Default for AuroraNpcState {
    fn default() -> Self {
        Self {
            short_term_memory: ShortTermMemory::default(),
            intention: vec![(IntentKind::Idle, 1.0)],
            emotional_state: EmotionalState::default(),
        }
    }
}

/// AURORA's per-NPC extension-state overlay (BL-82 EM-4.2f, spec §1.5),
/// keyed by the mirrored NPC's [`crate::NetUid`] value. See the module doc
/// comment for the Offline-fallback invariant: this map is empty while
/// [`crate::AiExecutionMode::Offline`] and holds exactly one
/// [`AuroraNpcState::default`] neutral entry per mirrored NPC otherwise.
///
/// This is a plain server-side [`Resource`] (not yet a replicated
/// `Component`) — nothing in this task sends it to a client; a future
/// HUD/debug render (BL-83+) can add that separately.
#[derive(Resource, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AuroraOverlay(pub HashMap<u64, AuroraNpcState>);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_term_memory_starts_empty() {
        let mem = ShortTermMemory::default();
        assert!(mem.is_empty());
        assert_eq!(mem.len(), 0);
    }

    #[test]
    fn short_term_memory_evicts_oldest_past_capacity() {
        let mut mem = ShortTermMemory::default();
        for i in 0..(ShortTermMemory::CAPACITY + 3) {
            mem.push(format!("event {i}"));
        }
        assert_eq!(mem.len(), ShortTermMemory::CAPACITY);
        let kept: Vec<&String> = mem.iter().collect();
        // The three oldest ("event 0..2") were evicted; the ring keeps the
        // most recent CAPACITY entries in order.
        assert_eq!(kept.first().map(|s| s.as_str()), Some("event 3"));
        assert_eq!(
            kept.last().map(|s| s.as_str()),
            Some(format!("event {}", ShortTermMemory::CAPACITY + 2).as_str())
        );
    }

    #[test]
    fn intent_kind_default_is_idle() {
        assert_eq!(IntentKind::default(), IntentKind::Idle);
    }

    #[test]
    fn mood_kind_default_is_neutral() {
        assert_eq!(MoodKind::default(), MoodKind::Neutral);
    }

    #[test]
    fn emotional_state_default_is_neutral_zero_intensity() {
        let state = EmotionalState::default();
        assert_eq!(state.mood, MoodKind::Neutral);
        assert_eq!(state.intensity, 0.0);
    }

    /// Pins the EXACT documented neutral placeholder (spec §1.5): empty
    /// memory, `Idle`-only intention at weight 1.0, neutral mood at zero
    /// intensity — this is what every populated `AuroraOverlay` entry must
    /// equal until BL-83 writes real data.
    #[test]
    fn npc_state_default_matches_documented_neutral_placeholder() {
        let state = AuroraNpcState::default();
        assert!(state.short_term_memory.is_empty());
        assert_eq!(state.intention, vec![(IntentKind::Idle, 1.0)]);
        assert_eq!(state.emotional_state, EmotionalState {
            mood: MoodKind::Neutral,
            intensity: 0.0,
        });
    }

    #[test]
    fn overlay_default_is_empty() {
        assert!(AuroraOverlay::default().0.is_empty());
    }

    /// The nested enums stay a closed, RON-round-trippable set (matches every
    /// other Phase-4 schema's convention — `AiExecutionMode`,
    /// `FallbackPolicy`).
    #[test]
    fn intent_and_mood_round_trip_through_ron() {
        for intent in [
            IntentKind::Idle,
            IntentKind::Patrol,
            IntentKind::Flee,
            IntentKind::Seek,
            IntentKind::Socialize,
            IntentKind::Guard,
        ] {
            let text = ron::to_string(&intent).expect("serializes");
            let back: IntentKind = ron::from_str(&text).expect("deserializes");
            assert_eq!(back, intent);
        }
        for mood in [
            MoodKind::Neutral,
            MoodKind::Content,
            MoodKind::Wary,
            MoodKind::Afraid,
            MoodKind::Hostile,
            MoodKind::Curious,
        ] {
            let text = ron::to_string(&mood).expect("serializes");
            let back: MoodKind = ron::from_str(&text).expect("deserializes");
            assert_eq!(back, mood);
        }
    }
}
