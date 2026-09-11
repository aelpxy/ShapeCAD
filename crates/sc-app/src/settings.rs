//! Persisted user preferences.
//!
//! Interface scale, appearance, and which display to open on. Kept deliberately
//! separate from the document: how big you like the text, whether you work in
//! the dark, and which screen you use are properties of your machine rather than
//! of the part.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Which appearance the user asked for.
///
/// A request, not a result. `System` becomes one of the other two once the
/// window system has been asked, which is the only place that answer exists.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Appearance {
    /// Follow the desktop's own light or dark setting.
    #[default]
    System,
    Light,
    Dark,
}

impl Appearance {
    pub(crate) const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];

    /// Resolves to an actual palette.
    ///
    /// `system` is what the window system reports, which is `None` wherever it
    /// declines to say. Light is the fallback: it is the scheme the application
    /// was designed in, so it is the one guaranteed to be legible.
    pub(crate) fn resolve(self, system: Option<crate::theme::Scheme>) -> crate::theme::Scheme {
        match self {
            Self::Light => crate::theme::Scheme::Light,
            Self::Dark => crate::theme::Scheme::Dark,
            Self::System => system.unwrap_or(crate::theme::Scheme::Light),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct Settings {
    /// Interface zoom, multiplied onto the display's own scale factor.
    ///
    /// `None` means "not chosen yet": the first run derives a sensible value
    /// from the display instead of guessing 1.0 and rendering unreadably small
    /// on a 4K panel.
    pub(crate) ui_scale: Option<f32>,

    /// Name of the display to open on, as `--displays` prints it.
    ///
    /// `None` leaves the choice to the window system, which is the only option
    /// on Wayland unless a display is named: there a client is told neither
    /// which output it is on nor where it is, so the compositor decides and the
    /// application has no say.
    #[serde(default)]
    pub(crate) display: Option<String>,

    /// Light, dark, or whatever the desktop is set to.
    #[serde(default)]
    pub(crate) appearance: Appearance,

    /// Whether the tutorial has been finished or dismissed.
    ///
    /// It runs once, unasked, on a first launch. Offering it behind a menu item
    /// means the people who most need it are the least likely to find it.
    #[serde(default)]
    pub(crate) tutorial_seen: bool,
}

fn path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME").map_or_else(
        || std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")),
        |x| Some(PathBuf::from(x)),
    )?;
    Some(base.join("shapecad").join("settings.json"))
}

/// Narrowest and widest interface zoom that will be honoured.
///
/// The same range `AppState::set_ui_scale` clamps a keyboard zoom to. Anything
/// outside it in the file is not a preference, it is damage: a zoom of zero
/// divides the layout by nothing and one of a thousand leaves a single glyph
/// filling the window, in both cases with no way to reach the control that
/// would put it back.
const MIN_UI_SCALE: f32 = 0.75;
const MAX_UI_SCALE: f32 = 3.0;

impl Settings {
    /// Reads preferences, falling back to defaults on any problem.
    ///
    /// A corrupt settings file should never stop the application starting.
    pub(crate) fn load() -> Self {
        path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map_or_else(Self::default, |t| Self::parse(&t))
    }

    /// Preferences from the text of a settings file.
    ///
    /// Corrupt covers more than "not JSON". A file can parse perfectly and still
    /// hold a zoom of zero, or one of `1e30`, whether because something else
    /// wrote it, an edit went wrong, or a future version used the field
    /// differently. Those reach the window as a zoom factor and are as fatal to
    /// starting up as a syntax error, so a value outside the range the interface
    /// offers is discarded here rather than trusted.
    fn parse(text: &str) -> Self {
        let mut settings: Self = serde_json::from_str(text).unwrap_or_default();
        settings.ui_scale = settings
            .ui_scale
            .filter(|s| s.is_finite() && (MIN_UI_SCALE..=MAX_UI_SCALE).contains(s));
        // An empty name matches no monitor, so it is a choice that can never be
        // honoured. Treated as "let the window system decide", which is what an
        // absent one means.
        settings.display = settings.display.filter(|d| !d.trim().is_empty());
        settings
    }

    /// Writes preferences, ignoring failure.
    ///
    /// Losing a zoom preference is not worth interrupting the user over.
    pub(crate) fn save(&self) {
        let Some(p) = path() else { return };
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = serde_json::to_string_pretty(self) {
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

    /// A monitor width is whatever the window system says it is, and a
    /// compositor that has not mapped the surface yet can say something absurd.
    /// Whatever comes back has to be a usable zoom.
    #[test]
    fn an_impossible_display_still_yields_a_usable_zoom() {
        for width in [1, 7, u32::MAX / 2, u32::MAX] {
            let scale = auto_scale(width, 1.0);
            assert!(
                scale.is_finite() && (1.0..=3.0).contains(&scale),
                "{width}px wide gave a zoom of {scale}"
            );
        }
    }
}

#[cfg(test)]
mod file_tests {
    use super::{Appearance, Settings};

    /// The documented promise: a corrupt file must never stop the application
    /// starting. Reading it has to answer with defaults for every shape of
    /// corrupt, not just for text that is not JSON.
    #[test]
    fn nothing_in_a_corrupt_file_stops_the_application_starting() {
        for text in [
            "",
            "{",
            "not json at all",
            "[1, 2, 3]",
            "null",
            "42",
            r#"{"ui_scale": "large"}"#,
            r#"{"appearance": "Sepia"}"#,
            r#"{"ui_scale": 1.5, "display": 7}"#,
            "\u{0}\u{1}\u{2}",
        ] {
            let settings = Settings::parse(text);
            assert_eq!(
                settings.appearance,
                Appearance::System,
                "{text:?} did not fall back to defaults"
            );
            assert!(settings.ui_scale.is_none(), "{text:?} kept a zoom");
        }
    }

    /// Valid JSON of the wrong shape is the case that gets missed: it parses, so
    /// nothing rejects it, and the value goes straight to the window as a zoom
    /// factor. A zoom of zero lays the whole interface out at no size, and there
    /// is no way back from it because the control that would fix it is part of
    /// the interface.
    #[test]
    fn a_zoom_the_interface_could_not_offer_is_discarded() {
        for bad in ["0", "0.0", "-2", "1e30", "1e-30", "3.5", "0.5"] {
            let text = format!("{{ \"ui_scale\": {bad} }}");
            assert!(
                Settings::parse(&text).ui_scale.is_none(),
                "a zoom of {bad} was kept"
            );
        }
        for good in ["0.75", "1.0", "1.5", "3.0"] {
            let text = format!("{{ \"ui_scale\": {good} }}");
            assert!(
                Settings::parse(&text).ui_scale.is_some(),
                "a zoom of {good} was thrown away"
            );
        }
    }

    /// A display nobody can be on is the same as not having chosen one.
    #[test]
    fn an_empty_display_name_is_no_choice_at_all() {
        assert!(Settings::parse(r#"{ "display": "" }"#).display.is_none());
        assert!(Settings::parse(r#"{ "display": "  " }"#).display.is_none());
        assert_eq!(
            Settings::parse(r#"{ "display": "rdp-0" }"#)
                .display
                .as_deref(),
            Some("rdp-0")
        );
    }

    /// What the application writes has to be what it reads back, or a
    /// preference is lost on every second launch.
    #[test]
    fn a_settings_file_round_trips() {
        let settings = Settings {
            ui_scale: Some(1.5),
            display: Some("rdp-0".to_string()),
            appearance: Appearance::Dark,
            tutorial_seen: true,
        };
        let text = serde_json::to_string_pretty(&settings).expect("serialises");
        let back = Settings::parse(&text);
        assert_eq!(back.ui_scale, settings.ui_scale);
        assert_eq!(back.display, settings.display);
        assert_eq!(back.appearance, settings.appearance);
        assert!(back.tutorial_seen);
    }
}

#[cfg(test)]
mod appearance_tests {
    use super::Appearance;
    use crate::theme::Scheme;

    /// An explicit choice wins over whatever the desktop is doing. That is the
    /// whole point of offering one.
    #[test]
    fn an_explicit_choice_ignores_the_desktop() {
        for system in [None, Some(Scheme::Light), Some(Scheme::Dark)] {
            assert_eq!(Appearance::Light.resolve(system), Scheme::Light);
            assert_eq!(Appearance::Dark.resolve(system), Scheme::Dark);
        }
    }

    #[test]
    fn following_the_system_follows_the_system() {
        assert_eq!(Appearance::System.resolve(Some(Scheme::Dark)), Scheme::Dark);
        assert_eq!(
            Appearance::System.resolve(Some(Scheme::Light)),
            Scheme::Light
        );
    }

    /// Several Wayland compositors never report a colour scheme. Having nothing
    /// to follow is not an error, and it must not leave the interface unstyled.
    #[test]
    fn an_unanswered_system_falls_back_to_light() {
        assert_eq!(Appearance::System.resolve(None), Scheme::Light);
    }

    /// A settings file written before these preferences existed must load, and
    /// must not silently pin the user to one scheme.
    #[test]
    fn an_older_settings_file_defaults_to_following_the_system() {
        let older = r#"{ "ui_scale": 1.5 }"#;
        let loaded: super::Settings = serde_json::from_str(older).expect("older settings load");
        assert_eq!(loaded.appearance, Appearance::System);
        assert_eq!(loaded.ui_scale, Some(1.5));
        assert!(
            !loaded.tutorial_seen,
            "an existing user would never be shown the guide"
        );
    }

    /// And once it has been seen, that has to survive a round trip, or the guide
    /// reappears on every launch.
    #[test]
    fn having_seen_the_guide_round_trips() {
        let mut settings = super::Settings::default();
        assert!(!settings.tutorial_seen, "it has to run on a first launch");
        settings.tutorial_seen = true;

        let text = serde_json::to_string(&settings).expect("serialises");
        let back: super::Settings = serde_json::from_str(&text).expect("deserialises");
        assert!(back.tutorial_seen);
    }

    #[test]
    fn every_appearance_is_offered() {
        assert_eq!(Appearance::ALL.len(), 3);
        assert!(Appearance::ALL.contains(&Appearance::default()));
    }
}
