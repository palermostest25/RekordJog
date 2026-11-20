use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// User settings. Every field has a default so old settings files keep
/// loading after upgrades.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Multiplier on top of the calibrated jog resolution when scratching.
    pub scratch_sensitivity: f64,
    /// Multiplier for pitch bend / search with the jog wheel.
    pub bend_sensitivity: f64,
    /// Flip the tempo fader direction.
    pub invert_tempo: bool,
    /// Flip the browse knob direction.
    pub invert_browse: bool,
    /// Start with vinyl (scratch) mode on.
    pub vinyl_default: bool,
    /// Filter/Low knobs start in Filter (true) or Low EQ (false) mode.
    pub knob_default_filter: bool,
    /// How strongly Fade FX opens the filter (0..1).
    pub fadefx_depth: f64,
    /// control id -> action id, only for controls the user changed.
    pub bindings: BTreeMap<String, String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            scratch_sensitivity: 1.0,
            bend_sensitivity: 1.0,
            invert_tempo: true,
            invert_browse: false,
            vinyl_default: true,
            knob_default_filter: true,
            fadefx_depth: 0.8,
            bindings: BTreeMap::new(),
        }
    }
}

impl Config {
    pub fn sanitized(mut self) -> Self {
        let clamp = |v: f64, lo: f64, hi: f64, def: f64| if v.is_finite() { v.clamp(lo, hi) } else { def };
        self.scratch_sensitivity = clamp(self.scratch_sensitivity, 0.1, 4.0, 1.0);
        self.bend_sensitivity = clamp(self.bend_sensitivity, 0.1, 4.0, 1.0);
        self.fadefx_depth = clamp(self.fadefx_depth, 0.0, 1.0, 0.8);
        self
    }
}
