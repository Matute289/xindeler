//! BL-82 EM-5.12 — the Escape/pause menu + a Video/Graphics settings tab.
//!
//! Ports legacy `voxygen`'s esc menu (`voxygen/src/hud/esc_menu.rs`) + the
//! Video pane of its settings window (`voxygen/src/hud/settings_window/
//! video.rs`) into the Bevy client, SCOPED to what this epic needs: a centred
//! pause panel with a **Resume** button and a **Video** section exposing the
//! three graphics toggles that already exist as real, startup-read fields on
//! [`xindeler_app::GraphicsSettings`] — `ssao`, `taa`, and the
//! `shadow_cascades` count (see `crate::light::spawn_light_rig` /
//! `crate::camera::spawn_camera`).
//!
//! ## Deliberately out of scope (documented, not silently skipped)
//! Legacy's esc menu also had Character Selection / Report Bug / Logout / Quit
//! buttons, and its settings window had Interface/Gameplay/Controls/Sound/
//! Video/Language tabs. This module ships ONLY Resume + the Video graphics
//! trio — the slice this epic (and Matías's live shadow-flicker investigation,
//! which needs an in-game way to toggle exactly these three) requires. The
//! standalone Controls rebinding screen already exists separately
//! (`crate::controls_screen`, `HudWindow::Controls`); folding it in as a tab
//! here is future scope.
//!
//! ## Live application (the whole point for the flicker investigation)
//! [`apply_graphics_settings`] reconciles the live camera to match
//! [`xindeler_app::GraphicsSettings`] whenever it changes: it inserts/removes
//! [`ScreenSpaceAmbientOcclusion`]/[`TemporalAntiAliasing`] on the camera — so
//! flipping SSAO or TAA changes the RUNNING render config immediately, no
//! restart, no renderer-plugin rebuild. Every change is persisted to
//! `settings.ron` (and the graphics `tier` is forced to
//! [`GraphicsTier::Custom`] so a hand-edited toggle isn't clobbered by a preset
//! on next load — see [`GraphicsSettings::sanitize`]).
//!
//! ## Shadow cascades apply on RESTART, not live (BL-82 crash fix)
//! The **cascade COUNT** is the one exception to live-apply, and deliberately
//! so. Changing `num_cascades` on an ALREADY-RUNNING directional light — by any
//! means, whether re-`insert`ing a new `CascadeShadowConfig` on the existing
//! sun OR despawning and respawning the sun entity — reliably aborts the client
//! from a `bevy_light` internal:
//!
//! ```text
//! thread 'Compute Task Pool' panicked at bevy_light-0.19.0/src/lib.rs:477:
//! index out of bounds: the len is 1 but the index is 1
//! ```
//!
//! Root cause (traced through the actual panicking system, not just the line):
//! `bevy_light::check_dir_light_mesh_visibility` keeps a **persistent**
//! `Local<Parallel<Vec<Vec<Entity>>>>` of per-cascade visibility scratch
//! queues. Each frame its `for_each_init` only `resize`s that scratch to the
//! current cascade count *on the worker threads rayon actually schedules work
//! onto*; threads left idle this pass keep the PREVIOUS frame's (shorter) Vec.
//! The collect loop then iterates ALL ever-touched thread-locals and indexes
//! each at the new cascade index — so the first frame the count *increases*
//! (e.g. 1 -> 2), any thread carrying a stale length-1 queue is indexed at [1]
//! and panics. Because that stale state lives in a per-SYSTEM `Local` (not on
//! the light entity), respawning the sun does not reset it — only a fresh app
//! start begins with an empty `Local`, which is why a cascade count picked at
//! `crate::light::spawn_light_rig` time is always safe while a live change is
//! not. Bevy is pinned at `=0.19.0` from crates.io (not a fork), so we fix this
//! from our side by NOT reconfiguring cascades on the live light: the setting
//! still cycles + persists, and takes effect on the next launch. SSAO and TAA
//! stay fully live.
//!
//! Compiled only under `listen-server`/`net-client`, matching every other
//! `xindeler_ui`-consuming screen module in this crate.

use bevy::{
    anti_alias::taa::TemporalAntiAliasing,
    core_pipeline::prepass::DepthPrepass,
    ecs::schedule::common_conditions::{not, resource_changed},
    pbr::ScreenSpaceAmbientOcclusion,
    prelude::*,
    render::camera::{MipBias, TemporalJitter},
};
use xindeler_app::{GraphicsTier, XindelerSettings};
use xindeler_input::{ActionState, GameInput};
use xindeler_ui::{
    button::{Activate, button_bundle},
    hud_state::{HudAction, HudState, HudWindow},
    panel::panel_bundle,
    theme::{HudFonts, HudTheme},
    zlayer,
};

use crate::{camera::MainCamera, chat::text_input_focused, targeting::hard_lock_active};

/// Effective shadow-cascade range (matches `crate::light::spawn_light_rig`'s
/// own `clamp(1, 4)`): cycling the toggle wraps within this.
const MIN_SHADOW_CASCADES: u8 = 1;
const MAX_SHADOW_CASCADES: u8 = 4;

/// Installs the esc/pause menu: spawns the (hidden) panel at `Startup`, opens/
/// closes it on Escape, keeps its visibility + toggle labels synced, and
/// applies graphics changes live.
pub struct EscMenuPlugin;

impl Plugin for EscMenuPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Startup,
            spawn_esc_menu.after(xindeler_ui::theme::init_theme),
        )
        .add_systems(
            Update,
            (
                // Reads `ActionState` — order after the frame's real input
                // resolution (same fix as every other `ActionState`-reading
                // toggle in this crate). Gated on `!text_input_focused` so
                // Escape while typing in chat blurs the chat box instead of
                // opening the pause menu (`chat::blur_chat_input_on_escape`
                // owns that). ALSO gated on `!hard_lock_active` (BL-82
                // EM-5.19 Phase 3, finalizing the seam P2 explicitly
                // deferred): while a hard lock is active, Escape ONLY clears
                // it (`targeting::clear_hard_lock_on_escape`, ordered
                // `.after(toggle_esc_menu)` so it reads this SAME frame's
                // pre-clear lock state — see that system's own doc comment
                // for the full ordering argument) and must NOT also open/
                // close the pause menu on that same press. Also ordered
                // `.after(targeting::release_invalid_hard_lock)`
                // (bevy-migration-reviewer finding on PR #120): that system
                // lives in `MirrorSet`, which has no inherited ordering vs.
                // this plain `Update` system (only `MirrorSet -> GameplaySet`
                // is chained), so without this edge "the locked target
                // dies/leaves range AND Escape is pressed the same frame"
                // would race `hard_lock_active`'s read against the auto-
                // release's `HardLock` write with no scheduling guarantee.
                // Ordered BEFORE `apply_hud_actions` so the open/close it
                // requests lands the SAME frame (the atomic cursor-free
                // chain `cursor::update_cursor_free` relies on).
                toggle_esc_menu
                    .after(xindeler_input::InputResolveSet)
                    .after(crate::targeting::release_invalid_hard_lock)
                    .before(xindeler_ui::hud_state::apply_hud_actions)
                    .run_if(not(text_input_focused))
                    .run_if(not(hard_lock_active)),
                // After `apply_hud_actions` so the panel's `Visibility` matches
                // the window state THIS frame (no one-frame open lag).
                sync_esc_menu_visibility.after(xindeler_ui::hud_state::apply_hud_actions),
                sync_graphics_labels,
                // Reconcile the live render config whenever settings change.
                apply_graphics_settings.run_if(resource_changed::<XindelerSettings>),
            ),
        );
    }
}

/// The pause panel root (its [`Visibility`] mirrors
/// `HudState::is_open(HudWindow::EscMenu)`).
#[derive(Component)]
struct EscMenuRoot;

/// The three live-tunable graphics controls this menu exposes.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum GraphicsControl {
    Ssao,
    Taa,
    ShadowCascades,
}

/// Tags a graphics toggle button so [`sync_graphics_labels`] can relabel it
/// from the current [`XindelerSettings`].
#[derive(Component, Clone, Copy)]
struct GraphicsControlButton(GraphicsControl);

/// Escape is the universal "back out" key: it closes whatever window is open
/// (the pause menu, Diary, Inventory, Map, …), and only summons the pause menu
/// when NOTHING is open. This mirrors legacy `voxygen`'s `Show::toggle_windows`
/// exactly (Escape closes any open window; with nothing open it opens the esc
/// menu). Making it universal here — rather than relying on each window to
/// carry its own Escape-close handler — means every current AND future
/// [`HudWindow`] gets Escape-to-close for free (only `Map` had its own handler
/// before; Diary/Inventory/Social/Crafting/Controls had none, so Escape did
/// nothing with them open — ecs-design-reviewer finding). Writing a
/// [`HudAction::CloseWindow`] is idempotent ([`HudState::close`] sets `None`),
/// so `map_view::close_full_map_on_escape` also firing on the same frame is
/// harmless — both just close the (one) open window.
///
/// `pub(crate)` (BL-82 EM-5.19 Phase 3): so
/// `targeting::clear_hard_lock_on_escape` can name it in an explicit
/// `.after(toggle_esc_menu)` ordering edge — see that system's doc comment for
/// why the ORDER (not just the `hard_lock_active` run-condition gate above) is
/// load-bearing for "Escape-while-locked clears the lock without also opening
/// the pause menu on the same press."
pub(crate) fn toggle_esc_menu(
    action_state: Res<ActionState>,
    hud_state: Res<HudState>,
    mut actions: MessageWriter<HudAction>,
) {
    if !action_state.just_pressed(GameInput::Escape) {
        return;
    }
    if hud_state.any_window_open() {
        actions.write(HudAction::CloseWindow);
    } else {
        actions.write(HudAction::ToggleWindow(HudWindow::EscMenu));
    }
}

/// Mirrors [`HudState`]'s open window onto the panel's [`Visibility`] (read-
/// only w.r.t. [`HudAction`] — `apply_hud_actions` is the one applier, exactly
/// like `controls_screen::sync_window_visibility`).
fn sync_esc_menu_visibility(
    hud_state: Res<HudState>,
    mut root: Query<&mut Visibility, With<EscMenuRoot>>,
) {
    let Ok(mut visibility) = root.single_mut() else {
        return;
    };
    *visibility = if hud_state.is_open(HudWindow::EscMenu) {
        Visibility::Visible
    } else {
        Visibility::Hidden
    };
}

/// Spawns the centred pause panel: a "Game Menu" title, a Resume button, a
/// "Video" section header, and one labelled toggle button per
/// [`GraphicsControl`].
///
/// BL-82 EM-5.17/5.18 click-routing fix: `EscMenuRoot` is a full-screen modal
/// backdrop exactly like `DiaryWindowRoot`/`InventoryWindowRoot`/`FullMapRoot`,
/// but — unlike the diary — it was spawned without `GlobalZIndex(
/// zlayer::MODAL_WINDOWS)`. Left at the default z-partition (0), it sat
/// BELOW the always-on ambient HUD chrome once that chrome gained its own
/// higher z-index this phase (hotbar/orbs = `ORBS_ACTION_BAR_PARTY_MINIMAP`
/// =20): wherever the pause panel visually overlapped the hotbar,
/// `bevy_ui` picking (which resolves the highest z-partition first)
/// routed clicks to that ambient chrome instead of the pause menu underneath
/// — i.e. opening ESC did not actually block hotbar interaction where they
/// overlapped.
fn spawn_esc_menu(
    mut commands: Commands,
    theme: Res<HudTheme>,
    fonts: Res<HudFonts>,
    settings: Res<XindelerSettings>,
) {
    let theme: HudTheme = *theme;
    commands
        .spawn((
            EscMenuRoot,
            Visibility::Hidden,
            GlobalZIndex(zlayer::MODAL_WINDOWS),
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..Default::default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.55)),
        ))
        .with_children(|screen| {
            let mut panel_entity = screen.spawn(panel_bundle(&theme));
            let row_gap_px = theme.spacing.sm;
            panel_entity.entry::<Node>().and_modify(move |mut node| {
                node.flex_direction = FlexDirection::Column;
                node.row_gap = Val::Px(row_gap_px);
                node.min_width = Val::Px(360.0);
                node.align_items = AlignItems::Stretch;
            });
            panel_entity.with_children(|panel| {
                heading(panel, &fonts, &theme, "Game Menu", 28.0);

                panel
                    .spawn(button_bundle(&theme, &fonts, "Resume"))
                    .observe(handle_resume_click);

                heading(panel, &fonts, &theme, "Video", 20.0);

                for control in [
                    GraphicsControl::Ssao,
                    GraphicsControl::Taa,
                    GraphicsControl::ShadowCascades,
                ] {
                    spawn_graphics_row(panel, &theme, &fonts, &settings, control);
                }

                // Restart-vs-live honesty: SSAO/TAA apply immediately; the
                // shadow-cascade COUNT is read once at startup by
                // `crate::light::spawn_light_rig` (changing it on the live sun
                // aborts the client — see the module doc), so it is flagged
                // "(restart)" and applies on the next launch.
                panel.spawn((
                    Text(
                        "SSAO and anti-aliasing apply immediately. Shadow cascades apply on \
                         restart."
                            .to_owned(),
                    ),
                    TextFont {
                        font: bevy::text::FontSource::Handle(fonts.body.clone()),
                        font_size: bevy::text::FontSize::Px(13.0),
                        ..Default::default()
                    },
                    TextColor(theme.palette.text_muted),
                ));
            });
        });
}

/// A section/title heading line inside the panel.
fn heading(
    panel: &mut ChildSpawnerCommands,
    fonts: &HudFonts,
    theme: &HudTheme,
    text: &str,
    size: f32,
) {
    panel.spawn((
        Text(text.to_owned()),
        TextFont {
            font: bevy::text::FontSource::Handle(fonts.title.clone()),
            font_size: bevy::text::FontSize::Px(size),
            ..Default::default()
        },
        TextColor(theme.palette.text),
    ));
}

/// One graphics row: a name label + a value button that cycles the setting
/// (`On`/`Off`, or the cascade count) on click.
fn spawn_graphics_row(
    panel: &mut ChildSpawnerCommands,
    theme: &HudTheme,
    fonts: &HudFonts,
    settings: &XindelerSettings,
    control: GraphicsControl,
) {
    panel
        .spawn(Node {
            flex_direction: FlexDirection::Row,
            column_gap: Val::Px(theme.spacing.sm),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::SpaceBetween,
            ..Default::default()
        })
        .with_children(|row| {
            row.spawn((
                Text(control_name(control).to_owned()),
                TextFont {
                    font: bevy::text::FontSource::Handle(fonts.body.clone()),
                    font_size: bevy::text::FontSize::Px(16.0),
                    ..Default::default()
                },
                TextColor(theme.palette.text),
                Node {
                    width: Val::Px(180.0),
                    ..Default::default()
                },
            ));
            row.spawn(button_bundle(
                theme,
                fonts,
                &control_value_label(control, &settings.graphics),
            ))
            .insert(GraphicsControlButton(control))
            .observe(
                move |_activate: On<Activate>, mut settings: ResMut<XindelerSettings>| {
                    cycle_control(control, &mut settings);
                    // A hand-edited toggle must survive a reload — a non-Custom
                    // tier's preset would clobber it on next `sanitize()`.
                    settings.graphics.tier = GraphicsTier::Custom;
                    if let Err(err) = settings.save() {
                        error!(
                            "esc menu: failed to persist settings.ron after a graphics change: \
                             {err}"
                        );
                    }
                },
            );
        });
}

/// Resume closes the pause menu (routes through the generic `HudAction` bus,
/// exactly like every other window's close control).
fn handle_resume_click(_activate: On<Activate>, mut actions: MessageWriter<HudAction>) {
    actions.write(HudAction::CloseWindow);
}

/// Advances a control's value in [`XindelerSettings`] (booleans flip; the
/// cascade count cycles `1→2→3→4→1`).
fn cycle_control(control: GraphicsControl, settings: &mut XindelerSettings) {
    let g = &mut settings.graphics;
    match control {
        GraphicsControl::Ssao => g.ssao = !g.ssao,
        GraphicsControl::Taa => g.taa = !g.taa,
        GraphicsControl::ShadowCascades => {
            g.shadow_cascades = if g.shadow_cascades >= MAX_SHADOW_CASCADES {
                MIN_SHADOW_CASCADES
            } else {
                g.shadow_cascades + 1
            };
        },
    }
}

fn control_name(control: GraphicsControl) -> &'static str {
    match control {
        GraphicsControl::Ssao => "SSAO",
        GraphicsControl::Taa => "Anti-aliasing (TAA)",
        // "(restart)": unlike SSAO/TAA this does NOT apply live — see the
        // module-level "Shadow cascades apply on RESTART" doc for the
        // `bevy_light` crash it avoids. The new count is read at startup by
        // `crate::light::spawn_light_rig`.
        GraphicsControl::ShadowCascades => "Shadow cascades (restart)",
    }
}

fn control_value_label(
    control: GraphicsControl,
    graphics: &xindeler_app::GraphicsSettings,
) -> String {
    match control {
        GraphicsControl::Ssao => on_off(graphics.ssao).to_owned(),
        GraphicsControl::Taa => on_off(graphics.taa).to_owned(),
        GraphicsControl::ShadowCascades => graphics
            .shadow_cascades
            .clamp(MIN_SHADOW_CASCADES, MAX_SHADOW_CASCADES)
            .to_string(),
    }
}

fn on_off(value: bool) -> &'static str { if value { "On" } else { "Off" } }

/// Refreshes every graphics toggle button's label from the current settings
/// (so a click's effect is immediately visible). Gated on `is_changed` so it
/// only walks the buttons when settings actually change.
fn sync_graphics_labels(
    settings: Res<XindelerSettings>,
    buttons: Query<(&GraphicsControlButton, &Children)>,
    mut texts: Query<&mut Text>,
) {
    if !settings.is_changed() {
        return;
    }
    for (button, children) in &buttons {
        let label = control_value_label(button.0, &settings.graphics);
        for &child in children {
            if let Ok(mut text) = texts.get_mut(child)
                && text.0 != label
            {
                text.0 = label.clone();
            }
        }
    }
}

/// Reconciles the live CAMERA render components (SSAO/TAA) to match
/// [`XindelerSettings`] — this is what makes those two toggles apply WITHOUT a
/// restart. Idempotent: it only inserts/removes a component when the live state
/// doesn't already match, so re-running it on any settings change (e.g. a
/// controls rebind that also saves settings) is harmless.
///
/// It deliberately does NOT touch the sun's `CascadeShadowConfig`: changing
/// the cascade COUNT on the live directional light aborts the client from a
/// `bevy_light` internal (stale per-thread visibility scratch — see this
/// module's "Shadow cascades apply on RESTART" doc). The cascade count is read
/// once at startup by `crate::light::spawn_light_rig`, so a changed value is
/// simply persisted here and takes effect on the next launch.
fn apply_graphics_settings(
    settings: Res<XindelerSettings>,
    mut commands: Commands,
    cameras: Query<
        (
            Entity,
            Has<ScreenSpaceAmbientOcclusion>,
            Has<TemporalAntiAliasing>,
        ),
        With<MainCamera>,
    >,
) {
    let g = &settings.graphics;
    for (camera, has_ssao, has_taa) in &cameras {
        match (g.ssao, has_ssao) {
            (true, false) => {
                commands
                    .entity(camera)
                    .insert(ScreenSpaceAmbientOcclusion::default());
            },
            (false, true) => {
                commands
                    .entity(camera)
                    .remove::<ScreenSpaceAmbientOcclusion>();
            },
            _ => {},
        }
        match (g.taa, has_taa) {
            (true, false) => {
                // `TemporalAntiAliasing` `#[require]`s `TemporalJitter`,
                // `MipBias`, `DepthPrepass` and `MotionVectorPrepass`, all of
                // which Bevy adds automatically on this runtime insert (the
                // explicit `DepthPrepass` here is belt-and-suspenders, and
                // idempotent — Bevy no-ops a duplicate).
                commands
                    .entity(camera)
                    .insert((DepthPrepass, TemporalAntiAliasing::default()));
            },
            (false, true) => {
                // Removing ONLY `TemporalAntiAliasing` is NOT a clean "TAA off"
                // state: `TemporalJitter`'s sub-pixel projection offset (which
                // `bevy_render` keeps applying while the component is present)
                // freezes at its last value, and `MipBias`'s texture-sharpening
                // bias stays applied — so the picture would keep a permanent
                // jitter offset + sharpen (bevy-migration-reviewer finding).
                // For Matías's shadow-flicker A/B this matters: "TAA off" must
                // reach a real no-TAA baseline. Drop the jitter + mip bias too.
                // `DepthPrepass`/`MotionVectorPrepass` are left resident (no
                // visual residue, and other passes may want the depth prepass);
                // they cost a little GPU until restart — an accepted trade for
                // a live toggle.
                commands
                    .entity(camera)
                    .remove::<TemporalAntiAliasing>()
                    .remove::<TemporalJitter>()
                    .remove::<MipBias>();
            },
            _ => {},
        }
    }
    // NOTE: the shadow-cascade COUNT is intentionally NOT reconciled here.
    // Re-inserting a `CascadeShadowConfig` with a different `num_cascades` on
    // the live sun (or respawning the sun) crashes `bevy_light` — see the
    // module doc. It is applied at startup by `crate::light::spawn_light_rig`
    // instead; here it is only persisted (by the caller) for the next launch.
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    /// BL-82 EM-5.17/5.18 click-routing fix regression: `EscMenuRoot` is a
    /// full-screen modal backdrop exactly like `DiaryWindowRoot`/
    /// `InventoryWindowRoot`/`FullMapRoot`, and pins that it now actually
    /// carries `GlobalZIndex(MODAL_WINDOWS)`, matching `diary.rs`'s
    /// `spawn_diary_window_uses_skill_tree_bg_and_modal_z_index` test.
    /// Before this fix `EscMenuRoot` had NO `GlobalZIndex` at all (default
    /// z-partition 0) — it sat BELOW the always-on ambient chrome once that
    /// chrome gained its own higher z-index this phase (hotbar/orbs =
    /// `ORBS_ACTION_BAR_PARTY_MINIMAP`=20): wherever the pause panel
    /// visually overlapped the hotbar, `bevy_ui` picking (highest
    /// z-partition first) routed clicks to that ambient chrome instead of
    /// the pause menu underneath — i.e. opening ESC did not actually block
    /// hotbar interaction where they overlapped.
    #[test]
    fn esc_menu_root_carries_the_modal_windows_z_index() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(HudTheme::default());
        app.insert_resource(HudFonts {
            title: Handle::default(),
            body: Handle::default(),
        });
        app.insert_resource(XindelerSettings::default());

        app.world_mut()
            .run_system_once(spawn_esc_menu)
            .expect("spawn_esc_menu runs");

        let world = app.world_mut();
        let z_index = world
            .query_filtered::<&GlobalZIndex, With<EscMenuRoot>>()
            .single(world)
            .expect("EscMenuRoot exists")
            .0;
        assert_eq!(z_index, zlayer::MODAL_WINDOWS);
    }

    /// Escape with nothing open requests the pause menu; Escape with ANY
    /// window open (the pause menu OR another panel like the Diary) requests a
    /// close — Escape is the universal back-out key (ecs-design-reviewer
    /// finding: it must close Diary/Inventory/etc., which had no Escape handler
    /// of their own, not just Map/EscMenu).
    #[test]
    fn escape_opens_and_closes_only_when_appropriate() {
        fn run(open: HudWindow) -> Vec<HudAction> {
            use xindeler_input::KeyMap;

            let mut app = App::new();
            app.init_resource::<HudState>();
            app.insert_resource(KeyMap::default());
            app.insert_resource(ActionState::default());
            app.init_resource::<ButtonInput<KeyCode>>();
            app.insert_resource(ButtonInput::<bevy::input::mouse::MouseButton>::default());
            app.add_message::<HudAction>();
            if open != HudWindow::None {
                app.world_mut().resource_mut::<HudState>().toggle(open);
            }
            // Drive Escape through the real resolver so `just_pressed` is set.
            let esc_key = app
                .world()
                .resource::<KeyMap>()
                .keyboard
                .get_binding(GameInput::Escape);
            if let Some(xindeler_input::KeyOrMouse::Key(key)) = esc_key {
                app.world_mut()
                    .resource_mut::<ButtonInput<KeyCode>>()
                    .press(key);
            }
            app.add_systems(
                Update,
                (
                    xindeler_input::action_state::update_action_state,
                    toggle_esc_menu,
                )
                    .chain(),
            );
            app.update();
            app.world_mut()
                .resource_mut::<Messages<HudAction>>()
                .drain()
                .collect()
        }

        assert_eq!(
            run(HudWindow::None),
            vec![HudAction::ToggleWindow(HudWindow::EscMenu)],
            "Escape with nothing open must summon the pause menu"
        );
        assert_eq!(
            run(HudWindow::EscMenu),
            vec![HudAction::CloseWindow],
            "Escape with the pause menu open must close it"
        );
        assert_eq!(
            run(HudWindow::Diary),
            vec![HudAction::CloseWindow],
            "Escape with another window (Diary) open must close it — Escape is the universal \
             back-out key, not just a pause-menu opener"
        );
    }

    /// BL-82 EM-5.19 Phase 3: while a hard lock is active, Escape must NOT
    /// open/close the pause menu at all —
    /// `targeting::clear_hard_lock_on_escape` (not exercised by this
    /// fixture; see `targeting.rs`'s own tests for that half) owns clearing
    /// the lock instead. Builds the REAL `.run_if(not(text_input_focused)).
    /// run_if(not(hard_lock_active))` chain
    /// (unlike `escape_opens_and_closes_only_when_appropriate` above, which
    /// calls the bare `toggle_esc_menu` function with no conditions) so this
    /// actually exercises the gating wiring, not just the predicate.
    #[test]
    fn escape_does_not_open_pause_menu_while_hard_lock_active() {
        use bevy::input_focus::InputFocus;
        use xindeler_input::KeyMap;

        use crate::targeting::HardLock;

        let mut app = App::new();
        app.init_resource::<HudState>();
        app.insert_resource(KeyMap::default());
        app.insert_resource(ActionState::default());
        app.init_resource::<ButtonInput<KeyCode>>();
        app.insert_resource(ButtonInput::<bevy::input::mouse::MouseButton>::default());
        // `text_input_focused` (the other run condition in this chain) reads
        // `Res<InputFocus>` unconditionally — it must be present even though
        // this test's whole point is the `hard_lock_active` gate, not chat
        // focus (`chat::tests` is what actually exercises the focused case).
        app.init_resource::<InputFocus>();
        app.add_message::<HudAction>();
        let locked = app.world_mut().spawn_empty().id();
        app.insert_resource(HardLock(Some(locked)));

        let esc_key = app
            .world()
            .resource::<KeyMap>()
            .keyboard
            .get_binding(GameInput::Escape);
        if let Some(xindeler_input::KeyOrMouse::Key(key)) = esc_key {
            app.world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .press(key);
        }
        app.add_systems(
            Update,
            (
                xindeler_input::action_state::update_action_state,
                toggle_esc_menu
                    .run_if(not(text_input_focused))
                    .run_if(not(hard_lock_active)),
            )
                .chain(),
        );
        app.update();

        let actions: Vec<HudAction> = app
            .world_mut()
            .resource_mut::<Messages<HudAction>>()
            .drain()
            .collect();
        assert!(
            actions.is_empty(),
            "Escape while a hard lock is active must not emit any HudAction — no pause-menu \
             open/close on the same press"
        );
    }

    /// Cycling each control walks the expected values: booleans flip, the
    /// cascade count wraps 4→1.
    #[test]
    fn cycling_controls_advances_the_values() {
        let mut settings = XindelerSettings::default();
        settings.graphics.ssao = true;
        cycle_control(GraphicsControl::Ssao, &mut settings);
        assert!(!settings.graphics.ssao, "SSAO toggles off");

        settings.graphics.taa = false;
        cycle_control(GraphicsControl::Taa, &mut settings);
        assert!(settings.graphics.taa, "TAA toggles on");

        settings.graphics.shadow_cascades = 3;
        cycle_control(GraphicsControl::ShadowCascades, &mut settings);
        assert_eq!(settings.graphics.shadow_cascades, 4);
        cycle_control(GraphicsControl::ShadowCascades, &mut settings);
        assert_eq!(
            settings.graphics.shadow_cascades, MIN_SHADOW_CASCADES,
            "the cascade count wraps 4 -> 1"
        );
    }

    /// [`apply_graphics_settings`] reconciles the live camera: enabling SSAO/
    /// TAA in settings inserts the components; disabling removes them — the
    /// "applies live, no restart" acceptance bar (headless: asserts the ECS
    /// reconciliation, which is exactly the state the render graph reads).
    #[test]
    fn apply_reconciles_camera_components_to_settings() {
        let mut app = App::new();
        app.insert_resource(XindelerSettings::default());
        let camera = app.world_mut().spawn(MainCamera).id();
        app.add_systems(Update, apply_graphics_settings);

        // Default settings (Ultra: ssao + taa on) -> both components inserted.
        app.update();
        assert!(
            app.world()
                .get::<ScreenSpaceAmbientOcclusion>(camera)
                .is_some(),
            "SSAO-on settings must insert the SSAO component live"
        );
        assert!(
            app.world().get::<TemporalAntiAliasing>(camera).is_some(),
            "TAA-on settings must insert the TAA component live"
        );

        // Turn both off -> both removed.
        {
            let mut settings = app.world_mut().resource_mut::<XindelerSettings>();
            settings.graphics.ssao = false;
            settings.graphics.taa = false;
        }
        app.update();
        assert!(
            app.world()
                .get::<ScreenSpaceAmbientOcclusion>(camera)
                .is_none(),
            "SSAO-off settings must remove the SSAO component live"
        );
        assert!(
            app.world().get::<TemporalAntiAliasing>(camera).is_none(),
            "TAA-off settings must remove the TAA component live"
        );
    }

    /// Disabling TAA must reach a CLEAN no-TAA baseline: the residual
    /// `TemporalJitter` (a frozen sub-pixel projection offset) and `MipBias`
    /// (texture sharpening) `TemporalAntiAliasing` pulls in must be removed
    /// too, not just the TAA node (bevy-migration-reviewer finding — this is
    /// the baseline Matías's shadow-flicker A/B relies on). Inserts the trio
    /// explicitly (deterministic, no reliance on `#[require]` auto-add in a
    /// headless app) and asserts all three are gone after a TAA-off reconcile.
    #[test]
    fn disabling_taa_clears_the_jitter_and_mip_bias_residue() {
        let mut app = App::new();
        let mut settings = XindelerSettings::default();
        settings.graphics.taa = false;
        app.insert_resource(settings);
        let camera = app
            .world_mut()
            .spawn((
                MainCamera,
                TemporalAntiAliasing::default(),
                TemporalJitter::default(),
                MipBias(-1.0),
            ))
            .id();
        app.add_systems(Update, apply_graphics_settings);

        app.update();

        assert!(
            app.world().get::<TemporalAntiAliasing>(camera).is_none(),
            "TAA node removed"
        );
        assert!(
            app.world().get::<TemporalJitter>(camera).is_none(),
            "the frozen jitter offset must be cleared for a clean no-TAA baseline"
        );
        assert!(
            app.world().get::<MipBias>(camera).is_none(),
            "the TAA mip-bias sharpening must be cleared for a clean no-TAA baseline"
        );
    }

    /// BL-82 crash regression: changing `shadow_cascades` must NOT touch the
    /// live sun's `CascadeShadowConfig`. Re-inserting a config with a
    /// different `num_cascades` on the running light (or respawning it) aborts
    /// the client from a `bevy_light` internal — the stale per-thread
    /// visibility scratch in `check_dir_light_mesh_visibility` (see the module
    /// doc). The count is applied at startup by `crate::light::spawn_light_rig`
    /// instead; here we assert the live-reconcile system leaves an existing
    /// cascade config byte-for-byte untouched even across a settings change,
    /// so the crash-triggering live mutation can never be reintroduced without
    /// this test failing.
    #[test]
    fn changing_shadow_cascades_does_not_mutate_the_live_sun() {
        use bevy::light::{CascadeShadowConfig, CascadeShadowConfigBuilder};

        let mut app = App::new();
        // Start at the Ultra default (4 cascades).
        app.insert_resource(XindelerSettings::default());
        // A stand-in for the live sun, carrying a 1-cascade config. If the
        // system ever live-reconciled cascades, a settings bump to a higher
        // count would grow `bounds` here — exactly the cross-frame count
        // INCREASE that crashes `bevy_light`.
        let sun_config = CascadeShadowConfigBuilder {
            num_cascades: 1,
            maximum_distance: 500.0,
            ..Default::default()
        }
        .build();
        let bounds_before = sun_config.bounds.len();
        assert_eq!(bounds_before, 1, "sanity: 1 cascade -> 1 bound");
        let sun = app.world_mut().spawn(sun_config).id();
        app.add_systems(Update, apply_graphics_settings);

        // First reconcile with the default settings.
        app.update();
        // Now change the cascade count (1 -> 3, the crash-prone INCREASE).
        {
            let mut settings = app.world_mut().resource_mut::<XindelerSettings>();
            settings.graphics.shadow_cascades = 3;
        }
        app.update();

        let after = app
            .world()
            .get::<CascadeShadowConfig>(sun)
            .expect("sun still carries its cascade config");
        assert_eq!(
            after.bounds.len(),
            bounds_before,
            "apply_graphics_settings must NOT reconfigure the live sun's cascade count — that \
             crashes bevy_light; the count is applied at startup instead"
        );
    }
}
