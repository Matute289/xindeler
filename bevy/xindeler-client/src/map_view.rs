//! BL-82 EM-5.5 — the map: an always-visible minimap + a toggle-able full
//! map screen, both reading the ONE-SHOT `NetMapData` broadcast
//! (`xindeler-sim-bridge::map::send_map_data_once`) for their background
//! image + site/POI markers, and the already-mirrored local player `NetPos`/
//! `NetOri` for the player arrow/marker. Follows `combat_hud`'s established
//! shape: real mirrored sim state in, themed `bevy_ui` widgets out, degrade
//! clean (no mirror data yet -> renders empty, never panics).
//!
//! ## v1 scope (documented, not silently dropped — the EM-5.1/5.2 convention)
//! Shipped: the map background image (a real runtime [`Image`] built from the
//! decoded `NetMapData` pixels — no placeholder/mock texture), site/quest
//! markers + named POIs (peaks/lakes) with hover tooltips, the player
//! position/heading arrow, a fixed-zoom fixed-north always-visible minimap,
//! and a full map screen (toggle `KeyM`/`Escape`, mouse-wheel zoom, left-drag
//! pan).
//!
//! Deferred (no data/primitive exists yet to build on — real gaps, not
//! oversights):
//! - **Group member dots**: needs `NetGroup` (EM-5.8, not landed at the time
//!   this shipped) — both map screens leave a documented gap rather than faking
//!   group data.
//! - **Difficulty badges**: legacy computes these from a PER-DUNGEON-KIND
//!   metadata table (`DungeonKindMeta`) the compact `MarkerKind` this mirror
//!   carries doesn't preserve — a real badge needs a NEW mirrored field, not
//!   just client-side math.
//! - **Waypoint marker**: `client::Client::waypoint()` returns an internal
//!   serialized-position `String` (not a structured position) — decoding that
//!   legacy format is out of scope here; a real waypoint marker needs a
//!   structured field added to a future mirror revision.
//! - **Real `.vox`/icon-atlas art**: EM-5.1's own module doc comment already
//!   defers the icon-atlas primitive to whichever screen needs it first;
//!   markers/POIs here use themed colour-swatch dots + hover tooltips, the same
//!   primitive `combat_hud`'s buff strip already established.
//!
//! Compiled only under the `listen-server`/`net-client` cargo features (the
//! only modes where `xindeler-protocol`'s `Net*` types are even linked — see
//! `main.rs`'s `#[cfg]`-gated module list), matching every other consumer
//! module in this crate.

use bevy::{
    asset::RenderAssetUsages,
    color::ColorToPacked,
    ecs::schedule::common_conditions::not,
    image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor},
    input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll},
    math::Rect,
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};
use xindeler_input::{ActionState, GameInput};
use xindeler_protocol::{
    NetLocalPlayer, NetMapData, NetMapMarker, NetMapPoi, NetOri, NetPos, wpos_to_screen_uv,
};
use xindeler_ui::{
    hud_state::{HudAction, HudState, HudWindow},
    theme::{HudFonts, HudTheme},
    tooltip::Tooltip,
};

use crate::chat::text_input_focused;

const MINIMAP_PANEL_PX: f32 = 160.0;
/// Fixed UV half-extent the minimap shows around the player (v1 has no zoom
/// control on the minimap — a follow-up, matching EM-5.1's own precedent of
/// deferring some interactive knobs).
///
/// This crops a `2 * MINIMAP_HALF_EXTENT` (~9%) slice of
/// [`xindeler_protocol::map::NetMapData`]'s background image into
/// [`MINIMAP_PANEL_PX`] on-screen pixels — see
/// [`xindeler_protocol::map::MAP_IMAGE_MAX_DIM`]'s own doc comment for the
/// BL-82 Phase 5 follow-up investigation that found this crop's resolution
/// requirement and that constant were previously uncorrelated (the
/// "pixelated/blurry minimap" bug: real under-resolution, not a sampler
/// filtering bug). Changing either this extent or the panel size without
/// re-checking that math risks reintroducing the same blockiness.
///
/// ## BL-82 EM-5.5 follow-up (2026-07-12): "too far away" / not enough zoom
/// A live play session (`record17.mov`) reported the always-on minimap
/// looking like it was viewed from too far away, wanting the "height"
/// lowered and more detail. The prior `0.06` (~12% of world width, a
/// ~1966-block radius for the shipped default 1024-chunk world) was
/// sanity-checked against the legacy `voxygen` minimap
/// (`voxygen/src/hud/minimap.rs`'s `minimap_zoom: 160.0` default —
/// `xindeler-old` ships the exact same untouched value): legacy's initial
/// zoom shows roughly 0.6% of world width, ~20x tighter than `0.06` was
/// here, confirming this minimap really was unusually zoomed out by
/// Veloren-family standards. Lowered to `0.045` (~9% of world width, a
/// ~1475-block radius): a meaningful but deliberately NOT extreme
/// "lower the height" (legacy's own minimap is further user-zoomable via
/// `+`/`-`, a real interactive knob this minimap still doesn't have —
/// stays deferred per this const's own first paragraph, not reintroduced
/// here). Chosen to respect the resolution ceiling this data source
/// actually has: `client::WorldData::map_image()` is inherently 1 pixel
/// per chunk (32 blocks) with no sub-chunk detail to extract, so
/// tightening the crop trades screen-pixel magnification for zoom —
/// `0.045` keeps the resulting magnification at ~1.74x, comfortably under
/// the `< 2.0` "acceptable" bound the Phase 5 pixelation fix established
/// (see the `minimap_crop_stays_close_to_native_resolution_for_the_default_world`
/// test below), rather than gambling right at that edge (`0.04` would
/// have landed at ~1.95x, too close for comfort). A genuinely
/// higher-resolution "local terrain" minimap — legacy's separate
/// real-time `show_voxel_map` sampling actual nearby block colours, not
/// this low-res world-map image — is a real, larger follow-up: flagged
/// here, not silently promised by this constant tweak.
const MINIMAP_HALF_EXTENT: f32 = 0.045;
const ARROW_SIZE_PX: f32 = 18.0;
const MARKER_DOT_PX: f32 = 8.0;
/// Side length of the SQUARE map viewport (where the map image/markers
/// actually render) — square deliberately: [`crop_rect_uv`] produces a
/// square UV crop (same `half_extent` on both axes), and the world/chunk
/// grid this codebase targets is square in practice (documented
/// simplification, `crop_rect_uv`'s own doc comment). A non-square viewport
/// would either letterbox (gaps either side — `ImageNode`'s default
/// `NodeImageMode::Auto` aspect-preserving fit) or distort (`Stretch`,
/// non-uniform scale) the map image relative to the marker dots positioned
/// against the SAME square crop math — a real bug this codebase hit live
/// (dots rendering outside the visibly-letterboxed image, inside the
/// now-Stretch-filled viewport). Square-to-square needs neither.
const FULL_MAP_VIEWPORT_PX: f32 = 480.0;
/// Height of the title row above the square viewport.
const FULL_MAP_TITLE_ROW_PX: f32 = 24.0;
/// `(panel width, panel height)` — the viewport plus its title row (the
/// panel's own padding is added on top by `panel`'s `Node`, not baked in
/// here).
const FULL_MAP_PANEL_PX: (f32, f32) = (
    FULL_MAP_VIEWPORT_PX,
    FULL_MAP_VIEWPORT_PX + FULL_MAP_TITLE_ROW_PX,
);
const MIN_ZOOM: f32 = 0.02;
const MAX_ZOOM: f32 = 1.0;
const ZOOM_STEP_FRACTION: f32 = 0.12;

/// Which on-screen viewport a [`MapMarkerDot`] belongs to — the minimap and
/// full map are cropped independently (different zoom/pan state), so the
/// same world marker needs an independently-positioned dot in each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ViewportKind {
    Minimap,
    FullMap,
}

/// Decoded [`NetMapData`], once it arrives — `texture` is `None` until then,
/// so every consuming system degrades clean (spec §3.2) rather than reading
/// stale/placeholder data.
#[derive(Resource, Default)]
struct MapData {
    texture: Option<Handle<Image>>,
    image_size: [u32; 2],
    world_size_chunks: [u32; 2],
    chunk_size_blocks: u32,
    markers: Vec<NetMapMarker>,
    pois: Vec<NetMapPoi>,
}

/// Full map's user-adjustable pan/zoom state. `zoom` is the UV half-extent
/// shown per axis (`MAX_ZOOM` = the whole map, smaller = more zoomed in);
/// `center` is the UV point the view is centered on, initially the map
/// center (re-centered onto the player the first time the window opens, see
/// [`recenter_full_map_on_open`]).
#[derive(Resource)]
struct FullMapView {
    zoom: f32,
    center: Vec2,
    has_been_opened: bool,
}

impl Default for FullMapView {
    fn default() -> Self {
        Self {
            zoom: MAX_ZOOM,
            center: Vec2::splat(0.5),
            has_been_opened: false,
        }
    }
}

#[derive(Component)]
struct MinimapViewport;
#[derive(Component)]
struct MinimapImage;
#[derive(Component)]
struct MinimapArrow;

#[derive(Component)]
struct FullMapRoot;
#[derive(Component)]
struct FullMapViewport;
#[derive(Component)]
struct FullMapImage;
#[derive(Component)]
struct FullMapPlayerMarker;

/// One dot spawned per marker/POI, per viewport (so the minimap and full map
/// each get their own independently-positioned copy). `uv` is the marker's
/// fixed world-space screen-UV (baked once at spawn — markers never move
/// during a session, unlike the player).
#[derive(Component)]
struct MapMarkerDot {
    uv: Vec2,
    viewport: ViewportKind,
}

/// Installs the whole map: minimap (always-on) + full map (toggle-able),
/// both reading [`MapData`] once [`NetMapData`] arrives.
pub struct MapViewPlugin;

impl Plugin for MapViewPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MapData>()
            .init_resource::<FullMapView>()
            .init_resource::<MinimapCropRes>()
            .init_resource::<FullMapCropRes>()
            .add_systems(
                Startup,
                spawn_map_screens.after(xindeler_ui::theme::init_theme),
            )
            .add_systems(
                Update,
                (
                    receive_map_data,
                    spawn_marker_dots.after(receive_map_data),
                    // Reads `ActionState` — must run after the frame's real
                    // input resolution (BL-82 EM-5.17 Phase 0, same fix as
                    // `diary::toggle_diary_window`/`controls_screen::
                    // toggle_controls_screen`). Also gated on
                    // `!text_input_focused` so typing "m" in the chat box
                    // doesn't ALSO open the full map.
                    toggle_full_map_window
                        .after(xindeler_input::InputResolveSet)
                        .run_if(not(text_input_focused)),
                    force_open_map_for_smoke_verification,
                    sync_full_map_visibility,
                    recenter_full_map_on_open,
                    sync_minimap,
                    // ecs-design-reviewer finding: both this system and
                    // `recenter_full_map_on_open` write `FullMapView`, and
                    // this one READS `view.center` to build the crop — on
                    // the very frame the map first opens, an unspecified
                    // ordering could render one frame of the un-recentered
                    // (map-geometric-center) crop before snapping to the
                    // player next frame. Purely cosmetic (self-corrects),
                    // but explicit ordering costs nothing and removes the
                    // ambiguity.
                    sync_full_map_view.after(recenter_full_map_on_open),
                    sync_marker_dot_positions
                        .after(sync_minimap)
                        .after(sync_full_map_view),
                ),
            );
    }
}

/// Debug-only, opt-in (`XINDELER_SMOKE_OPEN_MAP=1`) forcing of the full map
/// window open — the same "permanent, opt-in, gated by an env var read once"
/// convention `player_input.rs`'s own `XINDELER_CAMERA_FOCUS_PERF_LOG`/
/// `SmokeCameraCollisionPlugin` and `xindeler-sim-bridge`'s
/// `XINDELER_DEBUG_SPINUP_DIMENSION` already use. Exists purely so a
/// `--smoke-screenshot` capture can verify the full map screen (which
/// otherwise needs a live keypress to open) renders real site markers — not
/// part of the normal play flow.
fn force_open_map_for_smoke_verification(
    hud_state: Option<ResMut<HudState>>,
    mut done: Local<bool>,
) {
    if *done {
        return;
    }
    if std::env::var("XINDELER_SMOKE_OPEN_MAP").is_ok_and(|v| v != "0")
        && let Some(mut hud_state) = hud_state
    {
        hud_state.toggle(HudWindow::Map);
    }
    *done = true;
}

// ---------------------------------------------------------------------------
// Pure helpers (unit-tested independently of any ECS/Bevy app state)
// ---------------------------------------------------------------------------

/// A `[center - half_extent, center + half_extent]` window on one axis,
/// SLID (not independently clamped) to stay within `[0, 1]` — this keeps the
/// window's SIZE constant (the same zoom level) even near a world edge,
/// rather than shrinking it the way clamping `min`/`max` independently would.
/// A `half_extent` covering the whole `[0, 1]` axis collapses to exactly
/// `(0.0, 1.0)`.
fn slide_axis(center: f32, half_extent: f32) -> (f32, f32) {
    let mut lo = center - half_extent;
    let mut hi = center + half_extent;
    if hi - lo >= 1.0 {
        return (0.0, 1.0);
    }
    if lo < 0.0 {
        let d = -lo;
        lo += d;
        hi += d;
    }
    if hi > 1.0 {
        let d = hi - 1.0;
        lo -= d;
        hi -= d;
    }
    (lo, hi)
}

/// The 2D crop window (UV space) [`slide_axis`] produces, applied to both
/// axes independently. Known simplification (documented, not hidden): this
/// uses the SAME `half_extent` on both axes, which slightly stretches the
/// crop if the panel's pixel aspect ratio doesn't match the world's chunk
/// aspect ratio — acceptable for v1 since Xindeler/Veloren worlds are square
/// in practice.
fn crop_rect_uv(center: Vec2, half_extent: Vec2) -> Rect {
    let (min_x, max_x) = slide_axis(center.x, half_extent.x);
    let (min_y, max_y) = slide_axis(center.y, half_extent.y);
    Rect {
        min: Vec2::new(min_x, min_y),
        max: Vec2::new(max_x, max_y),
    }
}

/// Converts a UV-space crop window into the background image's PIXEL-space
/// sub-rect ([`bevy::ui::widget::ImageNode::rect`]'s own coordinate space).
fn crop_to_pixel_rect(crop: Rect, image_size: [u32; 2]) -> Rect {
    let dims = Vec2::new(image_size[0] as f32, image_size[1] as f32);
    Rect {
        min: crop.min * dims,
        max: crop.max * dims,
    }
}

/// Where a world point (already converted to screen-UV via
/// [`wpos_to_screen_uv`]) falls WITHIN a viewport currently showing `crop`,
/// in panel-local pixels. `None` if the point is outside the current crop
/// (so callers hide that marker rather than drawing it at a clamped, wrong
/// position).
fn uv_to_panel_px(point_uv: Vec2, crop: Rect, panel_size_px: Vec2) -> Option<Vec2> {
    let size = crop.max - crop.min;
    if size.x <= 0.0 || size.y <= 0.0 {
        return None;
    }
    let rel = (point_uv - crop.min) / size;
    if rel.x < 0.0 || rel.x > 1.0 || rel.y < 0.0 || rel.y > 1.0 {
        return None;
    }
    Some(rel * panel_size_px)
}

/// Extracts a screen-heading angle (radians, clockwise from north, matching
/// [`bevy_ui::UiTransform::rotation`]'s own "rotate clockwise" convention)
/// from a Bevy-space forward vector. North is `-Z` (Bevy's default per-object
/// forward) — consistent with [`wpos_to_screen_uv`]'s own north-up
/// convention, since the sim->Bevy position converter
/// (`xindeler-sim-bridge::sim_pos_to_bevy`) maps sim `+y` (north) to Bevy
/// `-z` too.
fn heading_from_forward(forward: Vec3) -> f32 { forward.x.atan2(-forward.z) }

/// [`heading_from_forward`] applied to a mirrored [`NetOri`] quaternion's
/// forward vector (Bevy's default object-forward, `-Z`).
fn heading_from_net_ori(ori: Quat) -> f32 { heading_from_forward(ori * Vec3::NEG_Z) }

/// A small solid-colour upward-pointing triangle, procedurally generated
/// (no new `assets/` file — matches `scene.rs::checkerboard_image`'s own
/// "generated in code" convention) — the player-position/heading icon shared
/// by both the minimap and the full map.
fn arrow_icon_image(color: Color) -> Image {
    const SIZE: u32 = 20;
    let rgba = color.to_srgba().to_u8_array();
    let mut data = vec![0u8; (SIZE * SIZE * 4) as usize];
    for y in 0..SIZE {
        // Widens going down the texture -> apex at the top (y=0), pointing
        // "up" (north) at zero rotation.
        let half_width = f32::from(y as u16) * 0.5;
        for x in 0..SIZE {
            let fx = f32::from(x as u16) - f32::from(SIZE as u16) / 2.0;
            if fx.abs() <= half_width {
                let idx = ((y * SIZE + x) * 4) as usize;
                data[idx..idx + 4].copy_from_slice(&rgba);
            }
        }
    }
    let mut image = Image::new(
        Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        mag_filter: ImageFilterMode::Nearest,
        min_filter: ImageFilterMode::Nearest,
        ..Default::default()
    });
    image
}

/// A themed colour-swatch dot for a [`NetMapMarker`]/[`NetMapPoi`] icon (the
/// same "no `.vox`/icon-atlas art yet" primitive `combat_hud`'s buff strip
/// uses). Civilized/quest sites get the theme accent; dungeon/hostile sites
/// get the danger tone; everything else (Tree/Unknown/POIs) gets a muted
/// neutral — a simple v1 split, not a per-kind palette (richer per-kind icons
/// are a follow-up needing the `.vox`-icon primitive).
fn marker_dot_color(theme: &HudTheme, kind: &common::map::MarkerKind) -> Color {
    use common::map::MarkerKind as K;
    match kind {
        K::Town | K::Character | K::ChapelSite | K::Bridge | K::GliderCourse => {
            theme.palette.accent
        },
        K::Castle
        | K::Cave
        | K::Gnarling
        | K::Terracotta
        | K::Adlet
        | K::Haniwa
        | K::DwarvenMine
        | K::Cultist
        | K::Sahagin
        | K::VampireCastle
        | K::Myrmidon => theme.palette.danger,
        K::Tree | K::Unknown => theme.palette.text_muted,
    }
}

// ---------------------------------------------------------------------------
// Systems
// ---------------------------------------------------------------------------

/// Spawns the always-on minimap (bottom-right) + the initially-hidden full
/// map overlay (centered).
fn spawn_map_screens(mut commands: Commands, theme: Res<HudTheme>, fonts: Res<HudFonts>) {
    // --- Minimap: bottom-right, always visible. ---
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                right: Val::Px(16.0),
                bottom: Val::Px(16.0),
                padding: UiRect::all(Val::Px(4.0)),
                border: UiRect::all(Val::Px(2.0)),
                border_radius: BorderRadius::all(Val::Px(theme.radius.md)),
                ..Default::default()
            },
            BackgroundColor(theme.palette.panel_bg),
            bevy::ui::BorderColor::all(theme.palette.panel_border),
        ))
        .with_children(|panel| {
            panel
                .spawn((MinimapViewport, Node {
                    width: Val::Px(MINIMAP_PANEL_PX),
                    height: Val::Px(MINIMAP_PANEL_PX),
                    overflow: bevy::ui::Overflow::clip(),
                    ..Default::default()
                }))
                .with_children(|viewport| {
                    viewport.spawn((
                        MinimapImage,
                        ImageNode {
                            // See `FullMapImage`'s own comment: fill the
                            // (square) viewport exactly, ignoring the
                            // source texture's own aspect.
                            image_mode: NodeImageMode::Stretch,
                            ..Default::default()
                        },
                        Node {
                            position_type: PositionType::Absolute,
                            width: Val::Px(MINIMAP_PANEL_PX),
                            height: Val::Px(MINIMAP_PANEL_PX),
                            ..Default::default()
                        },
                    ));
                    viewport.spawn((
                        MinimapArrow,
                        ImageNode::default(),
                        Node {
                            position_type: PositionType::Absolute,
                            width: Val::Px(ARROW_SIZE_PX),
                            height: Val::Px(ARROW_SIZE_PX),
                            left: Val::Px(MINIMAP_PANEL_PX / 2.0 - ARROW_SIZE_PX / 2.0),
                            top: Val::Px(MINIMAP_PANEL_PX / 2.0 - ARROW_SIZE_PX / 2.0),
                            ..Default::default()
                        },
                        UiTransform::IDENTITY,
                    ));
                });
        });

    // --- Full map: centered overlay, hidden until toggled. ---
    let (fw, fh) = FULL_MAP_PANEL_PX;
    commands
        .spawn((
            FullMapRoot,
            Visibility::Hidden,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Percent(50.0),
                left: Val::Percent(50.0),
                margin: UiRect {
                    top: Val::Px(-fh / 2.0),
                    left: Val::Px(-fw / 2.0),
                    ..Default::default()
                },
                width: Val::Px(fw),
                height: Val::Px(fh),
                padding: UiRect::all(theme.spacing.md_px()),
                border: UiRect::all(Val::Px(2.0)),
                border_radius: BorderRadius::all(Val::Px(theme.radius.md)),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(theme.spacing.xs),
                ..Default::default()
            },
            BackgroundColor(theme.palette.panel_bg),
            bevy::ui::BorderColor::all(theme.palette.panel_border),
        ))
        .with_children(|panel| {
            panel.spawn((
                Text("World Map (M / Esc to close, wheel to zoom, drag to pan)".to_owned()),
                TextFont {
                    font: bevy::text::FontSource::Handle(fonts.body.clone()),
                    font_size: bevy::text::FontSize::Px(14.0),
                    ..Default::default()
                },
                TextColor(theme.palette.text_muted),
            ));
            panel
                .spawn((FullMapViewport, Node {
                    width: Val::Px(FULL_MAP_VIEWPORT_PX),
                    height: Val::Px(FULL_MAP_VIEWPORT_PX),
                    overflow: bevy::ui::Overflow::clip(),
                    ..Default::default()
                }))
                .with_children(|viewport| {
                    viewport.spawn((
                        FullMapImage,
                        ImageNode {
                            // Fill the (square) viewport exactly regardless
                            // of the source texture's own pixel aspect —
                            // the default `Auto` mode aspect-fits instead,
                            // which (combined with a non-square background
                            // texture, e.g. a non-square world) would
                            // letterbox the image while marker dots (placed
                            // via the SAME square crop math) stay positioned
                            // against the full viewport, desyncing the two.
                            image_mode: NodeImageMode::Stretch,
                            ..Default::default()
                        },
                        Node {
                            position_type: PositionType::Absolute,
                            width: Val::Px(FULL_MAP_VIEWPORT_PX),
                            height: Val::Px(FULL_MAP_VIEWPORT_PX),
                            ..Default::default()
                        },
                    ));
                    viewport.spawn((
                        FullMapPlayerMarker,
                        ImageNode::default(),
                        Node {
                            position_type: PositionType::Absolute,
                            width: Val::Px(ARROW_SIZE_PX),
                            height: Val::Px(ARROW_SIZE_PX),
                            ..Default::default()
                        },
                        UiTransform::IDENTITY,
                        Visibility::Hidden,
                    ));
                });
        });
}

/// Decodes each arriving [`NetMapData`] into a real runtime [`Image`] +
/// stores the projected markers/POIs. `NetMapData` is a one-shot broadcast
/// (spec/`xindeler-sim-bridge::map` doc comment), so in practice this fires
/// at most once per session — but the system itself is not hardcoded to that
/// assumption (a later arrival, e.g. a future reconnect, just re-decodes).
fn receive_map_data(
    mut events: MessageReader<NetMapData>,
    mut images: ResMut<Assets<Image>>,
    mut data: ResMut<MapData>,
) {
    for net_data in events.read() {
        let Some(pixels) = net_data.decode_image() else {
            warn!("map_view: received a NetMapData with an undecodable image payload, dropping");
            continue;
        };
        let mut rgba = Vec::with_capacity(pixels.len() * 4);
        for p in &pixels {
            rgba.extend_from_slice(&[p[0], p[1], p[2], 255]);
        }
        let mut image = Image::new(
            Extent3d {
                width: net_data.image_size[0],
                height: net_data.image_size[1],
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            rgba,
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::RENDER_WORLD,
        );
        // Clamp-to-edge: a crop window pushed near a world edge (e.g. the
        // minimap near a corner) samples the edge colour rather than
        // wrapping to the opposite side of the map.
        image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
            address_mode_u: ImageAddressMode::ClampToEdge,
            address_mode_v: ImageAddressMode::ClampToEdge,
            mag_filter: ImageFilterMode::Linear,
            min_filter: ImageFilterMode::Linear,
            ..Default::default()
        });

        data.texture = Some(images.add(image));
        data.image_size = net_data.image_size;
        data.world_size_chunks = net_data.world_size_chunks;
        data.chunk_size_blocks = net_data.chunk_size_blocks;
        data.markers = net_data.markers.clone();
        data.pois = net_data.pois.clone();

        info!(
            markers = data.markers.len(),
            pois = data.pois.len(),
            "map_view: NetMapData received and decoded"
        );
    }
}

/// Spawns one [`MapMarkerDot`] per marker/POI, per viewport, the FIRST time
/// [`MapData`] gets real content (`Changed<MapData>` — cheap since it only
/// fires the handful of times `receive_map_data` actually writes something,
/// not every frame). Markers/POIs never move during a session, so their
/// screen-UV is baked once here rather than recomputed every frame.
fn spawn_marker_dots(
    mut commands: Commands,
    theme: Res<HudTheme>,
    data: Res<MapData>,
    minimap_viewport: Query<Entity, With<MinimapViewport>>,
    full_map_viewport: Query<Entity, With<FullMapViewport>>,
    existing_dots: Query<Entity, With<MapMarkerDot>>,
) {
    if !data.is_changed() || data.texture.is_none() {
        return;
    }
    // Rebuild from scratch — map data arrives at most a handful of times per
    // session (spec's one-shot contract), so despawn/respawn cost is
    // negligible (mirrors `combat_hud::sync_buff_strip`'s own rebuild-on-
    // change posture).
    for dot in &existing_dots {
        commands.entity(dot).despawn();
    }

    let Ok(minimap_viewport) = minimap_viewport.single() else {
        return;
    };
    let Ok(full_map_viewport) = full_map_viewport.single() else {
        return;
    };

    let mut spawn_one =
        |parent: Entity, uv: Vec2, viewport: ViewportKind, color: Color, tooltip: String| {
            commands.entity(parent).with_children(|p| {
                p.spawn((
                    MapMarkerDot { uv, viewport },
                    Node {
                        position_type: PositionType::Absolute,
                        width: Val::Px(MARKER_DOT_PX),
                        height: Val::Px(MARKER_DOT_PX),
                        border_radius: BorderRadius::all(Val::Px(MARKER_DOT_PX / 2.0)),
                        ..Default::default()
                    },
                    BackgroundColor(color),
                    bevy::picking::hover::Hovered(false),
                    Tooltip { text: tooltip },
                    Visibility::Hidden,
                ));
            });
        };

    for marker in &data.markers {
        let uv = wpos_to_screen_uv(
            Vec2::new(marker.wpos[0], marker.wpos[1]),
            data.world_size_chunks,
            data.chunk_size_blocks,
        );
        let color = marker_dot_color(&theme, &marker.kind);
        let name = marker
            .label
            .clone()
            .unwrap_or_else(|| format!("{:?}", marker.kind));
        let tooltip = if marker.is_quest {
            format!("{name} (quest)")
        } else {
            name
        };
        spawn_one(
            minimap_viewport,
            uv,
            ViewportKind::Minimap,
            color,
            tooltip.clone(),
        );
        spawn_one(full_map_viewport, uv, ViewportKind::FullMap, color, tooltip);
    }

    for poi in &data.pois {
        let uv = wpos_to_screen_uv(
            Vec2::new(poi.wpos[0], poi.wpos[1]),
            data.world_size_chunks,
            data.chunk_size_blocks,
        );
        let tooltip = format!("{} ({:?})", poi.name, poi.kind);
        spawn_one(
            minimap_viewport,
            uv,
            ViewportKind::Minimap,
            theme.palette.text_muted,
            tooltip.clone(),
        );
        spawn_one(
            full_map_viewport,
            uv,
            ViewportKind::FullMap,
            theme.palette.text_muted,
            tooltip,
        );
    }
}

/// [`GameInput::Map`] (`M` by default, rebindable) toggles the full map;
/// `Escape` closes it ONLY while it's the currently open window (scoped —
/// this doesn't claim generic Escape-closes-anything semantics for future
/// screens, which stays an open question for whichever epic wants to own it
/// generically).
///
/// BL-82 EM-5.17 Phase 0: the map toggle used to read the raw,
/// non-rebindable `ButtonInput<KeyCode>` via a hardcoded `KeyCode::KeyM` —
/// converted to [`ActionState`]/[`GameInput::Map`] so a rebind actually
/// takes effect. The `Escape`-closes check stays on raw `KeyCode` (out of
/// scope for this fix — no `GameInput::Escape` conversion attempted here).
fn toggle_full_map_window(
    action_state: Res<ActionState>,
    keys: Res<ButtonInput<KeyCode>>,
    hud_state: Res<HudState>,
    mut actions: MessageWriter<HudAction>,
) {
    if action_state.just_pressed(GameInput::Map) {
        actions.write(HudAction::ToggleWindow(HudWindow::Map));
    }
    if keys.just_pressed(KeyCode::Escape) && hud_state.is_open(HudWindow::Map) {
        actions.write(HudAction::CloseWindow);
    }
}

/// Mirrors [`HudState`]'s open window onto [`FullMapRoot`]'s [`Visibility`].
fn sync_full_map_visibility(
    hud_state: Res<HudState>,
    mut root: Query<&mut Visibility, With<FullMapRoot>>,
) {
    if !hud_state.is_changed() {
        return;
    }
    let Ok(mut visibility) = root.single_mut() else {
        return;
    };
    *visibility = if hud_state.is_open(HudWindow::Map) {
        Visibility::Visible
    } else {
        Visibility::Hidden
    };
}

/// The first time the full map opens in a session, re-centers its pan onto
/// the player's current position (rather than leaving it at the map's
/// geometric center) — every subsequent open keeps whatever pan/zoom the
/// player last left it at.
fn recenter_full_map_on_open(
    hud_state: Res<HudState>,
    data: Res<MapData>,
    player: Query<&NetPos, With<NetLocalPlayer>>,
    mut view: ResMut<FullMapView>,
) {
    if !hud_state.is_changed() || !hud_state.is_open(HudWindow::Map) || view.has_been_opened {
        return;
    }
    if let Ok(pos) = player.single() {
        let sim_xy = Vec2::new(pos.0.x, -pos.0.z);
        view.center = wpos_to_screen_uv(sim_xy, data.world_size_chunks, data.chunk_size_blocks);
    }
    view.has_been_opened = true;
}

/// Updates the minimap's cropped background + player arrow every frame from
/// the local player's real mirrored `NetPos`/`NetOri`. Degrades clean (spec
/// §3.2): with no map texture yet, or no local player mirrored yet, this is a
/// harmless no-op — the minimap just stays empty.
fn sync_minimap(
    data: Res<MapData>,
    player: Query<(&NetPos, Option<&NetOri>), With<NetLocalPlayer>>,
    mut arrow_handle: Local<Option<Handle<Image>>>,
    theme: Option<Res<HudTheme>>,
    mut assets: ResMut<Assets<Image>>,
    mut minimap_image: Query<&mut ImageNode, (With<MinimapImage>, Without<MinimapArrow>)>,
    mut minimap_arrow: Query<
        (&mut ImageNode, &mut UiTransform, &mut Visibility),
        With<MinimapArrow>,
    >,
    mut crop: ResMut<MinimapCropRes>,
) {
    let Some(theme) = theme else { return };

    let Ok((pos, ori)) = player.single() else {
        return;
    };
    let sim_xy = Vec2::new(pos.0.x, -pos.0.z);
    let center_uv = wpos_to_screen_uv(sim_xy, data.world_size_chunks, data.chunk_size_blocks);
    let extent = Vec2::splat(MINIMAP_HALF_EXTENT);
    let rect = crop_rect_uv(center_uv, extent);
    crop.0 = rect;

    if let Some(texture) = &data.texture
        && let Ok(mut node) = minimap_image.single_mut()
    {
        node.image = texture.clone();
        node.rect = Some(crop_to_pixel_rect(rect, data.image_size));
    }

    if let Ok((mut node, mut transform, mut visibility)) = minimap_arrow.single_mut() {
        // Lazily baked once a theme exists (Startup-order independent of
        // `spawn_map_screens`) — this system owns its OWN arrow handle,
        // deliberately separate from `sync_full_map_view`'s (a trivial 20x20
        // texture each, not worth coupling the two systems over).
        let handle =
            arrow_handle.get_or_insert_with(|| assets.add(arrow_icon_image(theme.palette.accent)));
        node.image = handle.clone();
        *visibility = Visibility::Inherited;
        let heading = ori.map_or(0.0, |o| heading_from_net_ori(o.0));
        transform.rotation = Rot2::radians(heading);
    }
}

/// Minimap's current UV crop, published each frame by [`sync_minimap`] for
/// [`sync_marker_dot_positions`] to read (avoids recomputing the player's
/// screen-UV twice per frame in two different systems).
#[derive(Resource, Default)]
struct MinimapCropRes(Rect);

/// Full map's current UV crop, published each frame by [`sync_full_map_view`]
/// (only meaningfully updated while the window is open).
#[derive(Resource, Default)]
struct FullMapCropRes(Rect);

/// Handles the full map's zoom (mouse wheel) + pan (left-drag) while open,
/// and updates its cropped background + player marker. A no-op while the
/// window is closed (no wasted per-frame math on the common "map closed"
/// case).
#[allow(clippy::too_many_arguments)]
fn sync_full_map_view(
    hud_state: Res<HudState>,
    data: Res<MapData>,
    player: Query<&NetPos, With<NetLocalPlayer>>,
    scroll: Res<AccumulatedMouseScroll>,
    motion: Res<AccumulatedMouseMotion>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    mut view: ResMut<FullMapView>,
    mut image_node: Query<&mut ImageNode, (With<FullMapImage>, Without<FullMapPlayerMarker>)>,
    mut player_marker: Query<
        (&mut ImageNode, &mut Node, &mut Visibility),
        With<FullMapPlayerMarker>,
    >,
    mut crop: ResMut<FullMapCropRes>,
    mut arrow_handle: Local<Option<Handle<Image>>>,
    mut assets: ResMut<Assets<Image>>,
    theme: Option<Res<HudTheme>>,
) {
    if !hud_state.is_open(HudWindow::Map) {
        return;
    }

    if scroll.delta.y.abs() > f32::EPSILON {
        let factor = 1.0 - scroll.delta.y.signum() * ZOOM_STEP_FRACTION;
        view.zoom = (view.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
    }
    if mouse_buttons.pressed(MouseButton::Left) {
        // Drag pans the view; dividing by the (square) viewport size converts
        // a pixel delta into a UV delta, scaled by the current zoom (screen
        // px covers `zoom*2` UV units across the viewport).
        let delta_uv = -motion.delta / FULL_MAP_VIEWPORT_PX * (view.zoom * 2.0);
        view.center = (view.center + delta_uv).clamp(Vec2::ZERO, Vec2::ONE);
    }

    let rect = crop_rect_uv(view.center, Vec2::splat(view.zoom));
    crop.0 = rect;

    if let Some(texture) = &data.texture
        && let Ok(mut node) = image_node.single_mut()
    {
        node.image = texture.clone();
        node.rect = Some(crop_to_pixel_rect(rect, data.image_size));
    }

    if let Some(theme) = theme {
        let handle =
            arrow_handle.get_or_insert_with(|| assets.add(arrow_icon_image(theme.palette.accent)));
        if let Ok((mut node, mut ui_node, mut visibility)) = player_marker.single_mut() {
            node.image = handle.clone();
            if let Ok(pos) = player.single() {
                let sim_xy = Vec2::new(pos.0.x, -pos.0.z);
                let player_uv =
                    wpos_to_screen_uv(sim_xy, data.world_size_chunks, data.chunk_size_blocks);
                match uv_to_panel_px(player_uv, rect, Vec2::splat(FULL_MAP_VIEWPORT_PX)) {
                    Some(px) => {
                        *visibility = Visibility::Inherited;
                        ui_node.left = Val::Px(px.x - ARROW_SIZE_PX / 2.0);
                        ui_node.top = Val::Px(px.y - ARROW_SIZE_PX / 2.0);
                    },
                    None => *visibility = Visibility::Hidden,
                }
            }
        }
    }
}

/// Repositions every [`MapMarkerDot`] each frame against its viewport's
/// current crop (published by [`sync_minimap`]/[`sync_full_map_view`]),
/// hiding any marker that has scrolled outside the visible crop rather than
/// drawing it at a wrong, clamped position.
fn sync_marker_dot_positions(
    minimap_crop: Res<MinimapCropRes>,
    full_map_crop: Res<FullMapCropRes>,
    mut dots: Query<(&MapMarkerDot, &mut Node, &mut Visibility)>,
) {
    for (dot, mut node, mut visibility) in &mut dots {
        let (crop, panel_size) = match dot.viewport {
            ViewportKind::Minimap => (minimap_crop.0, Vec2::splat(MINIMAP_PANEL_PX)),
            ViewportKind::FullMap => (full_map_crop.0, Vec2::splat(FULL_MAP_VIEWPORT_PX)),
        };
        match uv_to_panel_px(dot.uv, crop, panel_size) {
            Some(px) => {
                *visibility = Visibility::Inherited;
                node.left = Val::Px(px.x - MARKER_DOT_PX / 2.0);
                node.top = Val::Px(px.y - MARKER_DOT_PX / 2.0);
            },
            None => *visibility = Visibility::Hidden,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::{FRAC_PI_2, PI};

    use super::*;

    #[test]
    fn slide_axis_keeps_full_size_window_inside_bounds_near_edges() {
        // Near the low edge: window slides right, keeping its size.
        let (lo, hi) = slide_axis(0.01, 0.1);
        assert!((hi - lo - 0.2).abs() < 1e-6, "size must stay constant");
        assert!(lo >= 0.0 - 1e-6);

        // Near the high edge: window slides left.
        let (lo, hi) = slide_axis(0.99, 0.1);
        assert!((hi - lo - 0.2).abs() < 1e-6);
        assert!(hi <= 1.0 + 1e-6);

        // Comfortably in the middle: untouched.
        let (lo, hi) = slide_axis(0.5, 0.1);
        assert!((lo - 0.4).abs() < 1e-6);
        assert!((hi - 0.6).abs() < 1e-6);
    }

    #[test]
    fn slide_axis_collapses_to_full_range_when_half_extent_covers_it() {
        let (lo, hi) = slide_axis(0.5, 0.9);
        assert_eq!((lo, hi), (0.0, 1.0));
    }

    #[test]
    fn uv_to_panel_px_maps_crop_corners_to_panel_corners() {
        let crop = Rect {
            min: Vec2::new(0.25, 0.25),
            max: Vec2::new(0.75, 0.75),
        };
        let panel = Vec2::new(100.0, 200.0);

        let top_left = uv_to_panel_px(Vec2::new(0.25, 0.25), crop, panel).unwrap();
        assert!((top_left - Vec2::ZERO).length() < 1e-4);

        let bottom_right = uv_to_panel_px(Vec2::new(0.75, 0.75), crop, panel).unwrap();
        assert!((bottom_right - panel).length() < 1e-4);

        let center = uv_to_panel_px(Vec2::new(0.5, 0.5), crop, panel).unwrap();
        assert!((center - panel / 2.0).length() < 1e-4);
    }

    #[test]
    fn uv_to_panel_px_returns_none_outside_the_crop() {
        let crop = Rect {
            min: Vec2::new(0.25, 0.25),
            max: Vec2::new(0.75, 0.75),
        };
        assert!(uv_to_panel_px(Vec2::new(0.0, 0.0), crop, Vec2::splat(100.0)).is_none());
        assert!(uv_to_panel_px(Vec2::new(0.9, 0.9), crop, Vec2::splat(100.0)).is_none());
    }

    #[test]
    fn crop_to_pixel_rect_scales_uv_by_image_dimensions() {
        let crop = Rect {
            min: Vec2::new(0.0, 0.0),
            max: Vec2::new(0.5, 0.25),
        };
        let px = crop_to_pixel_rect(crop, [200, 400]);
        assert_eq!(px.min, Vec2::ZERO);
        assert_eq!(px.max, Vec2::new(100.0, 100.0));
    }

    /// Regression test for the "pixelated/blurry minimap" bug (BL-82 Phase 5
    /// follow-up): the minimap's magnification factor (on-screen px per
    /// SOURCE image px, for the shipped default 1024x1024-chunk world) must
    /// stay reasonably close to 1:1 — a large factor means the crop is
    /// stretching too few real source pixels across too many screen pixels,
    /// which is exactly the under-resolution bug no amount of correct linear
    /// filtering can hide (see `xindeler_protocol::map::MAP_IMAGE_MAX_DIM`'s
    /// doc comment for the full root-cause analysis). Before the fix (a 256
    /// cap against this same crop) this factor was ~5.2x; pins it under 2x
    /// going forward so a future change to `MINIMAP_HALF_EXTENT`,
    /// `MINIMAP_PANEL_PX`, or the shared resolution cap can't silently
    /// reintroduce the blockiness without this test catching it.
    #[test]
    fn minimap_crop_stays_close_to_native_resolution_for_the_default_world() {
        let source_px_in_crop =
            (2.0 * MINIMAP_HALF_EXTENT) * xindeler_protocol::MAP_IMAGE_MAX_DIM as f32;
        let magnification = MINIMAP_PANEL_PX / source_px_in_crop;
        assert!(
            magnification < 2.0,
            "minimap magnification is {magnification:.2}x — the crop is showing too few real \
             source pixels for the viewport size, which will look pixelated regardless of sampler \
             filtering"
        );
    }

    /// Regression test for the "minimap looks viewed from too far away" bug
    /// report (BL-82 EM-5.5 follow-up, `record17.mov`): pins the minimap to
    /// showing a meaningfully SMALLER slice of the world than the pre-fix
    /// `0.06` default (~12% of world width) — a future accidental revert of
    /// [`MINIMAP_HALF_EXTENT`] back toward that value regresses the exact
    /// zoomed-out-ness Matías reported, even though
    /// [`minimap_crop_stays_close_to_native_resolution_for_the_default_world`]
    /// alone wouldn't catch it (that test only guards the OTHER direction —
    /// zooming in too far and pixelating). Also asserts the real-world
    /// radius (in blocks) shown for the shipped default 1024-chunk world
    /// stays under the pre-fix radius, using the same
    /// `chunk-size (32 blocks) * world_size_chunks` math
    /// `xindeler_protocol::wpos_to_screen_uv` and the sim-bridge downsample
    /// use.
    ///
    /// `MINIMAP_HALF_EXTENT` and the numbers below are all `const`, so the
    /// comparisons are compile-time-foldable — clippy's
    /// `assertions_on_constants` rightly wants that expressed as a `const`
    /// assertion rather than a runtime one (a `let` binding wouldn't change
    /// that: the values are still const-derived). Kept as a `#[test]` (not a
    /// bare top-level `const _: () = assert!(...)`) so it shows up in normal
    /// test output alongside its sibling regression test above, but the
    /// `const { ... }` blocks mean a violation actually fails at COMPILE
    /// time, not just at test-run time — a stronger guard, not a weaker one.
    #[test]
    fn minimap_default_zoom_is_tighter_than_the_pre_fix_zoomed_out_value() {
        const PRE_FIX_HALF_EXTENT: f32 = 0.06;
        const DEFAULT_WORLD_CHUNKS: f32 = 1024.0;
        const CHUNK_SIZE_BLOCKS: f32 = 32.0;
        const RADIUS_BLOCKS: f32 = MINIMAP_HALF_EXTENT * DEFAULT_WORLD_CHUNKS * CHUNK_SIZE_BLOCKS;
        const PRE_FIX_RADIUS_BLOCKS: f32 =
            PRE_FIX_HALF_EXTENT * DEFAULT_WORLD_CHUNKS * CHUNK_SIZE_BLOCKS;

        const {
            assert!(
                MINIMAP_HALF_EXTENT < PRE_FIX_HALF_EXTENT,
                "MINIMAP_HALF_EXTENT regressed back toward (or past) the pre-fix zoomed-out \
                 default"
            );
        }
        const {
            assert!(
                RADIUS_BLOCKS < PRE_FIX_RADIUS_BLOCKS,
                "minimap world-radius (blocks) is not tighter than the pre-fix radius for the \
                 shipped default world"
            );
        }
    }

    /// The heading contract [`heading_from_forward`]'s doc comment promises:
    /// north (`-Z`) is zero; east (`+X`) is a quarter turn clockwise; south
    /// (`+Z`) is a half turn; west (`-X`) is a quarter turn counter-clockwise.
    #[test]
    fn heading_from_forward_matches_the_compass_convention() {
        assert!(
            (heading_from_forward(Vec3::NEG_Z) - 0.0).abs() < 1e-5,
            "north"
        );
        assert!(
            (heading_from_forward(Vec3::X) - FRAC_PI_2).abs() < 1e-5,
            "east"
        );
        assert!(
            (heading_from_forward(Vec3::NEG_X) + FRAC_PI_2).abs() < 1e-5,
            "west"
        );
        let south = heading_from_forward(Vec3::Z);
        assert!((south.abs() - PI).abs() < 1e-5, "south is +-pi");
    }

    /// Identity orientation (a mirrored entity with no rotation applied)
    /// faces Bevy's default forward (`-Z`) — i.e. north, heading 0 — so the
    /// arrow icon (baked pointing "up") needs no rotation at spawn.
    #[test]
    fn identity_net_ori_faces_north() {
        assert!((heading_from_net_ori(Quat::IDENTITY) - 0.0).abs() < 1e-5);
    }
}
