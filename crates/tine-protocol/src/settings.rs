//! Global application settings schema (`%APPDATA%\Tine\settings.json`).

#[cfg(feature = "serde")]
mod serde_settings {
    use crate::ids::SCHEMA_VERSION;
    use crate::presets::{FieldError, ValidationError};

    const fn legacy_onboarded() -> bool {
        true
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
    }

    impl Default for AppSettings {
        fn default() -> Self {
            Self {
                schema: SCHEMA_VERSION,
                selected_endpoint_id: None,
                monitor_device_id: None,
                monitor_enabled: false,
                pro_drawer_open: false,
                autostart: false,
                start_minimized: false,
                onboarding_completed: false,
                window_width: 1280,
                window_height: 800,
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

            Ok(())
        }
    }
}

#[cfg(feature = "serde")]
pub use serde_settings::AppSettings;
