//! Persisted user preferences.
//!
//! Only interface scale so far. Kept deliberately separate from the document:
//! how big you like the text is a property of your machine, not of the part.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub(crate) struct Settings {
    /// Interface zoom, multiplied onto the display's own scale factor.
    ///
    /// `None` means "not chosen yet": the first run derives a sensible value
    /// from the display instead of guessing 1.0 and rendering unreadably small
    /// on a 4K panel.
    pub(crate) ui_scale: Option<f32>,
}

fn path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME").map_or_else(
        || std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")),
        |x| Some(PathBuf::from(x)),
    )?;
    Some(base.join("shapecad").join("settings.json"))
}

impl Settings {
    /// Reads preferences, falling back to defaults on any problem.
    ///
    /// A corrupt settings file should never stop the application starting.
    pub(crate) fn load() -> Self {
        path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    /// Writes preferences, ignoring failure.
    ///
    /// Losing a zoom preference is not worth interrupting the user over.
    pub(crate) fn save(self) {
        let Some(p) = path() else { return };
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = serde_json::to_string_pretty(&self) {
            let _ = std::fs::write(p, text);
        }
    }
}

/// Interface zoom to use when the user has not chosen one.
///
/// Many Linux compositors, `WSLg` among them, report a scale factor of 1.0 even
/// on a 4K panel, which leaves every control about half the size it should be.
/// Where the display has already been accounted for, this trusts it and returns
/// 1.0; otherwise it derives a factor from the panel's width, treating 1920
/// logical pixels across as the reference.
#[must_use]
pub(crate) fn auto_scale(monitor_width_px: u32, native_points_per_pixel: f32) -> f32 {
    if native_points_per_pixel > 1.05 || monitor_width_px == 0 {
        return 1.0;
    }
    let raw = monitor_width_px as f32 / 1920.0;
    // Quarter steps: fractional scales below that just blur text.
    let stepped = (raw * 4.0).round() / 4.0;
    stepped.clamp(1.0, 3.0)
}

#[cfg(test)]
mod tests {
    use super::auto_scale;

    #[test]
    fn a_4k_panel_reporting_no_scaling_gets_doubled() {
        assert!((auto_scale(3840, 1.0) - 2.0).abs() < f32::EPSILON);
    }

    #[test]
    fn a_1080p_panel_is_left_alone() {
        assert!((auto_scale(1920, 1.0) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn a_compositor_that_already_scales_is_trusted() {
        // Reporting 2.0 means the platform has handled it; doubling again would
        // make everything enormous.
        assert!((auto_scale(3840, 2.0) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn an_ultrawide_is_capped_rather_than_absurd() {
        assert!(auto_scale(7680, 1.0) <= 3.0);
    }

    #[test]
    fn an_unknown_display_falls_back_to_no_scaling() {
        assert!((auto_scale(0, 1.0) - 1.0).abs() < f32::EPSILON);
    }
}
