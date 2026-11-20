//! Global application settings schema (`%APPDATA%\Tine\settings.json`).

#[cfg(feature = "serde")]
mod serde_settings {
    use crate::ids::SCHEMA_VERSION;
    use crate::params::DenoiseEngine;
    use crate::presets::{FieldError, ValidationError};

    const fn legacy_onboarded() -> bool {
        true
    }

    const fn default_true() -> bool {
        true
    }

    fn default_hotkey_mute() -> String {
        "Ctrl+Shift+M".to_owned()
    }

    fn default_hotkey_bypass() -> String {
        "Ctrl+Shift+B".to_owned()
    }

    /// Global user settings persisted in `%APPDATA%\Tine\settings.json`.
    #[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    #[serde(default)]
    pub struct AppSettings {
        /// Schema version (`1..=SCHEMA_VERSION`).
        pub schema: u32,
        /// Currently selected capture endpoint ID.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub selected_endpoint_id: Option<String>,
        /// Selected playback device ID for "Hear Myself" monitoring.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub monitor_device_id: Option<String>,
        /// Whether "Hear Myself" monitoring is enabled (not a DSP parameter).
        pub monitor_enabled: bool,
        /// Whether the Surgical Pro Rack drawer is expanded.
        pub pro_drawer_open: bool,
        /// Selected denoiser backend (`rnnoise`, `deepfilter`, or `off`).
        pub denoise_engine: DenoiseEngine,
        /// When `true`, parks the audio routing loop when no app uses the virtual mic (`false` = Keep Warm PTT).
        #[serde(default = "default_true")]
        pub park_when_unused: bool,
        /// Whether Tine launches on Windows login.
        pub autostart: bool,
        /// Whether Tine starts minimized to the system tray.
        pub start_minimized: bool,
        /// Whether the first-run onboarding flow has been completed.
        #[serde(default = "legacy_onboarded")]
        pub onboarding_completed: bool,
        /// Last window width in logical pixels.
        pub window_width: u32,
        /// Last window height in logical pixels.
        pub window_height: u32,
        /// Global Win32 hotkey to toggle mic mute (empty string disables).
        #[serde(default = "default_hotkey_mute")]
        pub hotkey_mute: String,
        /// Global Win32 hotkey to toggle DSP A/B bypass (empty string disables).
        #[serde(default = "default_hotkey_bypass")]
        pub hotkey_bypass: String,
    }

    impl Default for AppSettings {
        fn default() -> Self {
            Self {
                schema: SCHEMA_VERSION,
                selected_endpoint_id: None,
                monitor_device_id: None,
                monitor_enabled: false,
                pro_drawer_open: false,
                denoise_engine: DenoiseEngine::default(),
                park_when_unused: true,
                autostart: false,
                start_minimized: false,
                onboarding_completed: false,
                window_width: 1280,
                window_height: 800,
                hotkey_mute: default_hotkey_mute(),
                hotkey_bypass: default_hotkey_bypass(),
            }
        }
    }

    impl AppSettings {
        /// Validates schema version and window dimensions.
        pub fn validate(&self) -> Result<(), ValidationError> {
            if self.schema == 0 || self.schema > SCHEMA_VERSION {
                return Err(ValidationError::UnsupportedSchema {
                    actual: self.schema,
                    max_supported: SCHEMA_VERSION,
                });
            }
            let mut errors = Vec::new();
            if self.window_width < 640 || self.window_width > 7680 {
                errors.push(FieldError {
                    field: "window_width",
                    value: self.window_width as f32,
                    min: 640.0,
                    max: 7680.0,
                });
            }
            if self.window_height < 480 || self.window_height > 4320 {
                errors.push(FieldError {
                    field: "window_height",
                    value: self.window_height as f32,
                    min: 480.0,
                    max: 4320.0,
                });
            }
            if errors.is_empty() {
                Ok(())
            } else {
                Err(ValidationError::OutOfRange(errors))
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn onboarding_completed_defaults_and_legacy_deserialization() -> Result<(), serde_json::Error> {
            let defaults = AppSettings::default();
            assert!(!defaults.onboarding_completed);

            let legacy_json = r#"{"schema":1,"pro_drawer_open":true}"#;
            let legacy: AppSettings = serde_json::from_str(legacy_json)?;
            assert!(legacy.onboarding_completed);
            assert!(legacy.pro_drawer_open);

            let serialized = serde_json::to_string(&defaults)?;
            let round_tripped: AppSettings = serde_json::from_str(&serialized)?;
            assert!(!round_tripped.onboarding_completed);
            assert_eq!(round_tripped.denoise_engine, DenoiseEngine::Rnnoise);

            let dfn_json = r#"{"schema":1,"denoise_engine":"deepfilter"}"#;
            let dfn: AppSettings = serde_json::from_str(dfn_json)?;
            assert_eq!(dfn.denoise_engine, DenoiseEngine::DeepFilterNet3);
            assert!(dfn.park_when_unused, "missing park_when_unused must default to true");

            let ptt_json = r#"{"schema":1,"park_when_unused":false}"#;
            let ptt: AppSettings = serde_json::from_str(ptt_json)?;
            assert!(!ptt.park_when_unused);
            assert_eq!(ptt.hotkey_mute, "Ctrl+Shift+M");
            assert_eq!(ptt.hotkey_bypass, "Ctrl+Shift+B");

            let hk_json = r#"{"schema":1,"hotkey_mute":"Alt+M","hotkey_bypass":""}"#;
            let hk: AppSettings = serde_json::from_str(hk_json)?;
            assert_eq!(hk.hotkey_mute, "Alt+M");
            assert_eq!(hk.hotkey_bypass, "");

            Ok(())
        }
    }
}

#[cfg(feature = "serde")]
pub use serde_settings::AppSettings;
