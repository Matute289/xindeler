/// Number of Gaussian smoothing passes applied to terrain density fields.
/// 0 = smooth terrain disabled (use block physics only).
/// Set by the graphics system from TerrainSmoothingMode; read by physics.
#[derive(Clone, Copy, Debug, Default)]
pub struct SmoothTerrainSettings {
    pub passes: u8,
}
