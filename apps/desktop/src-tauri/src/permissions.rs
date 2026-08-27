//! Platform-aware permission report owned by the desktop/UI boundary.
//!
//! Native probes remain in the adapter crate that uses each permission; this
//! module only composes their results into the discriminated JSON shape the
//! settings UI consumes.

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "platform", rename_all = "snake_case")]
pub(crate) enum PermissionReport {
    #[cfg(any(target_os = "linux", test))]
    Linux {
        input_group: bool,
        uinput_writable: bool,
        fix_command: String,
    },
    #[cfg(any(not(target_os = "linux"), test))]
    Unsupported { os: String },
}

#[cfg(target_os = "linux")]
pub(crate) fn report() -> PermissionReport {
    from_linux(utter_inject::check_linux_permissions())
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn report() -> PermissionReport {
    PermissionReport::Unsupported {
        os: std::env::consts::OS.to_string(),
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
}
