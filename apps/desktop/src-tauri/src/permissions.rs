//! Platform-aware permission report owned by the desktop/UI boundary.
//!
//! Native probes remain in the adapter crate that uses each permission; this
//! module only composes their results into the discriminated JSON shape the
//! settings UI consumes.

use serde::Serialize;

#[cfg(any(target_os = "macos", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PermissionStatus {
    NotDetermined,
    Granted,
    Denied,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PermissionKind {
    Microphone,
    TextInjection,
}

impl PermissionKind {
    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        match value {
            "microphone" => Ok(Self::Microphone),
            "text_injection" => Ok(Self::TextInjection),
            other => Err(format!(
                "unknown permission kind '{other}': expected 'microphone' or 'text_injection'"
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "platform", rename_all = "snake_case")]
pub(crate) enum PermissionReport {
    #[cfg(any(target_os = "linux", test))]
    Linux {
        input_group: bool,
        uinput_writable: bool,
        fix_command: String,
    },
    #[cfg(any(target_os = "macos", test))]
    Macos {
        microphone: PermissionStatus,
        text_injection: PermissionStatus,
        bundle_id: String,
        microphone_reset_command: String,
        text_injection_reset_command: String,
    },
    #[cfg(any(not(any(target_os = "linux", target_os = "macos")), test))]
    Unsupported { os: String },
}

#[cfg(target_os = "linux")]
pub(crate) fn report() -> PermissionReport {
    from_linux(utter_inject::check_linux_permissions())
}

#[cfg(target_os = "macos")]
pub(crate) fn report() -> PermissionReport {
    macos_report(
        microphone_status(utter_audio::microphone_permission()),
        text_injection_status(utter_inject::text_injection_permission()),
    )
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) fn report() -> PermissionReport {
    PermissionReport::Unsupported {
        os: std::env::consts::OS.to_string(),
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn request(kind: PermissionKind) -> PermissionReport {
    match kind {
        PermissionKind::Microphone => {
            utter_audio::request_microphone_permission();
        }
        PermissionKind::TextInjection => {
            utter_inject::request_text_injection_permission();
        }
    }
    report()
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn request(_kind: PermissionKind) -> PermissionReport {
    report()
}

#[cfg(any(target_os = "macos", test))]
fn macos_report(
    microphone: PermissionStatus,
    text_injection: PermissionStatus,
) -> PermissionReport {
    PermissionReport::Macos {
        microphone,
        text_injection,
        bundle_id: utter_store::APP_IDENTIFIER.to_string(),
        microphone_reset_command: format!(
            "/usr/bin/tccutil reset Microphone {}",
            utter_store::APP_IDENTIFIER
        ),
        text_injection_reset_command: format!(
            "/usr/bin/tccutil reset Accessibility {}",
            utter_store::APP_IDENTIFIER
        ),
    }
}

#[cfg(any(target_os = "macos", test))]
fn settings_url(kind: PermissionKind) -> &'static str {
    match kind {
        PermissionKind::Microphone => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"
        }
        PermissionKind::TextInjection => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
        }
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn open_settings(kind: PermissionKind) -> Result<(), String> {
    let status = std::process::Command::new("/usr/bin/open")
        .arg(settings_url(kind))
        .status()
        .map_err(|error| format!("failed to open System Settings: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("System Settings exited with status {status}"))
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn open_settings(_kind: PermissionKind) -> Result<(), String> {
    Err("privacy settings can only be opened on macOS".to_string())
}

#[cfg(any(target_os = "macos", test))]
fn microphone_status(status: utter_audio::MicrophonePermission) -> PermissionStatus {
    match status {
        utter_audio::MicrophonePermission::NotDetermined => PermissionStatus::NotDetermined,
        utter_audio::MicrophonePermission::Granted => PermissionStatus::Granted,
        utter_audio::MicrophonePermission::Denied => PermissionStatus::Denied,
        utter_audio::MicrophonePermission::Unavailable => PermissionStatus::Unavailable,
    }
}

#[cfg(any(target_os = "macos", test))]
fn text_injection_status(status: utter_inject::TextInjectionPermission) -> PermissionStatus {
    match status {
        utter_inject::TextInjectionPermission::NotDetermined => PermissionStatus::NotDetermined,
        utter_inject::TextInjectionPermission::Granted => PermissionStatus::Granted,
        utter_inject::TextInjectionPermission::Denied => PermissionStatus::Denied,
        utter_inject::TextInjectionPermission::Unavailable => PermissionStatus::Unavailable,
    }
}

#[cfg(any(target_os = "linux", test))]
fn from_linux(report: utter_inject::LinuxPermissionReport) -> PermissionReport {
    PermissionReport::Linux {
        input_group: report.input_group,
        uinput_writable: report.uinput_writable,
        fix_command: report.fix_command,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_report_keeps_every_probe_value_and_has_a_discriminator() {
        let report = from_linux(utter_inject::LinuxPermissionReport {
            input_group: true,
            uinput_writable: false,
            fix_command: "fix it".to_string(),
        });
        let json = serde_json::to_value(report).unwrap();

        assert_eq!(json["platform"], "linux");
        assert_eq!(json["input_group"], true);
        assert_eq!(json["uinput_writable"], false);
        assert_eq!(json["fix_command"], "fix it");
    }

    #[test]
    fn unsupported_report_never_contains_linux_instructions() {
        let report = PermissionReport::Unsupported {
            os: "windows".to_string(),
        };
        let json = serde_json::to_string(&report).unwrap();

        assert!(json.contains(r#""platform":"unsupported""#));
        assert!(!json.contains("uinput"));
        assert!(!json.contains("usermod"));
    }

    #[test]
    fn macos_report_serializes_both_permission_statuses() {
        let report = macos_report(PermissionStatus::Granted, PermissionStatus::Denied);
        let json = serde_json::to_value(report).unwrap();

        assert_eq!(json["platform"], "macos");
        assert_eq!(json["microphone"], "granted");
        assert_eq!(json["text_injection"], "denied");
        assert_eq!(json["bundle_id"], utter_store::APP_IDENTIFIER);
        assert_eq!(
            json["microphone_reset_command"],
            format!(
                "/usr/bin/tccutil reset Microphone {}",
                utter_store::APP_IDENTIFIER
            )
        );
        assert_eq!(
            json["text_injection_reset_command"],
            format!(
                "/usr/bin/tccutil reset Accessibility {}",
                utter_store::APP_IDENTIFIER
            )
        );
    }

    #[test]
    fn macos_permission_adapters_preserve_every_native_status() {
        assert_eq!(
            microphone_status(utter_audio::MicrophonePermission::NotDetermined),
            PermissionStatus::NotDetermined
        );
        assert_eq!(
            text_injection_status(utter_inject::TextInjectionPermission::Unavailable),
            PermissionStatus::Unavailable
        );
    }

    #[test]
    fn rejects_unknown_permission_kind_before_any_platform_call() {
        assert_eq!(
            PermissionKind::parse("camera").unwrap_err(),
            "unknown permission kind 'camera': expected 'microphone' or 'text_injection'"
        );
    }

    #[test]
    fn each_macos_permission_opens_its_matching_privacy_pane() {
        assert!(settings_url(PermissionKind::Microphone).contains("Privacy_Microphone"));
        assert!(settings_url(PermissionKind::TextInjection).contains("Privacy_Accessibility"));
    }
}
