//! Allowlisted diagnostic report and the platform file-manager action exposed
//! by Advanced settings. Nothing in this module sends data anywhere.

use std::fs;
use std::path::Path;
use std::process::Command;

use serde::Serialize;
use utter_store::settings::EngineKind;
use utter_store::Settings;

use crate::logging::{logs_dir, redact};
use crate::permissions::PermissionReport;
#[cfg(any(target_os = "macos", test))]
use crate::permissions::PermissionStatus;

const LOG_FILE: &str = "utter.log";
const LOG_FILES: usize = 4;
const REPORT_LOG_LINES: usize = 200;

#[derive(Serialize)]
struct DiagnosticReport {
    app_version: &'static str,
    os: &'static str,
    arch: &'static str,
    whisper_metal_compiled: bool,
    profiles: Vec<ProfileSummary>,
    permissions: PermissionSummary,
    recent_logs: Vec<String>,
}

#[derive(Serialize)]
struct ProfileSummary {
    index: usize,
    engine: &'static str,
    model: Option<String>,
    preview_model: Option<String>,
}

#[derive(Serialize)]
#[serde(tag = "platform", rename_all = "snake_case")]
enum PermissionSummary {
    #[cfg(any(target_os = "linux", test))]
    Linux {
        input_group: bool,
        uinput_writable: bool,
    },
    #[cfg(any(target_os = "macos", test))]
    Macos {
        microphone: PermissionStatus,
        text_injection: PermissionStatus,
    },
    #[cfg(any(not(any(target_os = "linux", target_os = "macos")), test))]
    Unsupported { os: String },
}

impl From<PermissionReport> for PermissionSummary {
    fn from(value: PermissionReport) -> Self {
        match value {
            #[cfg(any(target_os = "linux", test))]
            PermissionReport::Linux {
                input_group,
                uinput_writable,
                ..
            } => Self::Linux {
                input_group,
                uinput_writable,
            },
            #[cfg(any(target_os = "macos", test))]
            PermissionReport::Macos {
                microphone,
                text_injection,
                ..
            } => Self::Macos {
                microphone,
                text_injection,
            },
            #[cfg(any(not(any(target_os = "linux", target_os = "macos")), test))]
            PermissionReport::Unsupported { os } => Self::Unsupported { os },
        }
    }
}

pub(crate) fn diagnostic_report(settings: &Settings) -> Result<String, String> {
    let recent_logs = logs_dir()
        .map(|dir| read_recent_logs(&dir, LOG_FILES, REPORT_LOG_LINES))
        .unwrap_or_default();
    let report = build_report(
        settings,
        crate::permissions::report(),
        recent_logs,
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        cfg!(all(target_os = "macos", feature = "whisper-metal")),
    );
    serde_json::to_string_pretty(&report).map_err(|error| error.to_string())
}

fn build_report(
    settings: &Settings,
    permissions: PermissionReport,
    recent_logs: Vec<String>,
    app_version: &'static str,
    os: &'static str,
    arch: &'static str,
    whisper_metal_compiled: bool,
) -> DiagnosticReport {
    DiagnosticReport {
        app_version,
        os,
        arch,
        whisper_metal_compiled,
        profiles: settings
            .profiles
            .iter()
            .enumerate()
            .map(|(index, profile)| ProfileSummary {
                index,
                engine: match profile.engine.active {
                    EngineKind::Whisper => "whisper",
                    EngineKind::Cloud => "cloud",
                    EngineKind::Sherpa => "sherpa",
                },
                model: match profile.engine.active {
                    EngineKind::Whisper => Some(profile.engine.whisper_model.clone()),
                    EngineKind::Cloud => Some(profile.engine.cloud.model.clone()),
                    EngineKind::Sherpa => profile.engine.sherpa_model.clone(),
                },
                preview_model: profile.draft.as_ref().map(|draft| draft.model.clone()),
            })
            .collect(),
        permissions: permissions.into(),
        recent_logs,
    }
}

fn read_recent_logs(dir: &Path, max_files: usize, max_lines: usize) -> Vec<String> {
    let mut paths = (1..max_files)
        .rev()
        .map(|suffix| dir.join(format!("{LOG_FILE}.{suffix}")))
        .collect::<Vec<_>>();
    paths.push(dir.join(LOG_FILE));

    let mut lines = Vec::new();
    for path in paths {
        let Ok(bytes) = fs::read(path) else {
            continue;
        };
        lines.extend(String::from_utf8_lossy(&bytes).lines().map(redact));
    }
    let remove = lines.len().saturating_sub(max_lines);
    lines.drain(..remove);
    lines
}

pub(crate) fn open_logs_directory() -> Result<(), String> {
    let dir = logs_dir()?;
    fs::create_dir_all(&dir).map_err(|error| error.to_string())?;

    #[cfg(target_os = "macos")]
    let mut command = Command::new("/usr/bin/open");
    #[cfg(target_os = "linux")]
    let mut command = Command::new("xdg-open");
    #[cfg(target_os = "windows")]
    let mut command = Command::new("explorer");
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    return Err("opening the log directory is unsupported on this platform".to_string());

    let status = command
        .arg(&dir)
        .status()
        .map_err(|error| format!("failed to open the log directory: {error}"))?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| format!("log directory opener exited with status {status}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_logs_are_bounded_and_read_in_chronological_order() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("utter.log.1"), "older-a\nolder-b\n").unwrap();
        fs::write(dir.path().join("utter.log"), "newer-a\nnewer-b\n").unwrap();

        assert_eq!(
            read_recent_logs(dir.path(), 2, 3),
            vec!["older-b", "newer-a", "newer-b"]
        );
    }

    #[test]
    fn report_schema_excludes_settings_secrets_and_personal_fields() {
        let mut settings = Settings::default();
        settings.profiles[0].id = "Andrey private".to_string();
        settings.profiles[0].recognition.custom_prompt = "private prompt".to_string();
        settings.refine.base_url = "https://host/v1?api_key=private".to_string();
        settings.dictionary.terms = vec!["private dictionary".to_string()];

        let permissions = PermissionReport::Macos {
            microphone: PermissionStatus::Granted,
            text_injection: PermissionStatus::Denied,
            bundle_id: "private bundle".to_string(),
            microphone_reset_command: "private microphone command".to_string(),
            text_injection_reset_command: "private injection command".to_string(),
        };
        let report = build_report(
            &settings,
            permissions,
            vec!["safe line".to_string()],
            "1.2.3",
            "macos",
            "aarch64",
            true,
        );
        let json = serde_json::to_string(&report).unwrap();

        assert!(json.contains("parakeet-tdt-110m-en"));
        for forbidden in [
            "Andrey private",
            "private prompt",
            "private dictionary",
            "api_key",
            "base_url",
            "reset_command",
            "private bundle",
            "private microphone command",
            "private injection command",
        ] {
            assert!(!json.contains(forbidden), "report leaked {forbidden}");
        }
    }
}
