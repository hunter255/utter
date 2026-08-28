//! Compile-time platform capabilities exposed to the settings UI.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlatformOs {
    #[cfg(any(target_os = "linux", test))]
    Linux,
    #[cfg(any(target_os = "macos", test))]
    Macos,
    #[cfg(any(not(any(target_os = "linux", target_os = "macos")), test))]
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InjectionCapability {
    Auto,
    ClipboardPaste,
    #[cfg(any(target_os = "linux", test))]
    Type,
    ClipboardOnly,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlatformCapabilities {
    pub os: PlatformOs,
    pub modifier_only_hotkeys: bool,
    pub injection_methods: Vec<InjectionCapability>,
}

pub fn capabilities() -> PlatformCapabilities {
    #[cfg(target_os = "linux")]
    let os = PlatformOs::Linux;
    #[cfg(target_os = "macos")]
    let os = PlatformOs::Macos;
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let os = PlatformOs::Other;

    for_os(os)
}

fn for_os(os: PlatformOs) -> PlatformCapabilities {
    use InjectionCapability::{Auto, ClipboardOnly, ClipboardPaste};

    match os {
        #[cfg(any(target_os = "linux", test))]
        PlatformOs::Linux => PlatformCapabilities {
            os,
            modifier_only_hotkeys: true,
            injection_methods: vec![
                Auto,
                ClipboardPaste,
                InjectionCapability::Type,
                ClipboardOnly,
            ],
        },
        #[cfg(any(target_os = "macos", test))]
        PlatformOs::Macos => PlatformCapabilities {
            os,
            modifier_only_hotkeys: false,
            injection_methods: vec![Auto, ClipboardPaste, ClipboardOnly],
        },
        #[cfg(any(not(any(target_os = "linux", target_os = "macos")), test))]
        PlatformOs::Other => PlatformCapabilities {
            os,
            modifier_only_hotkeys: false,
            injection_methods: vec![ClipboardOnly],
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_requires_a_base_key_and_hides_simulated_typing() {
        let report = for_os(PlatformOs::Macos);

        assert!(!report.modifier_only_hotkeys);
        assert_eq!(
            report.injection_methods,
            vec![
                InjectionCapability::Auto,
                InjectionCapability::ClipboardPaste,
                InjectionCapability::ClipboardOnly,
            ]
        );
    }

    #[test]
    fn linux_preserves_existing_hotkey_and_injection_choices() {
        let report = for_os(PlatformOs::Linux);

        assert!(report.modifier_only_hotkeys);
        assert!(report
            .injection_methods
            .contains(&InjectionCapability::Type));
    }

    #[test]
    fn capabilities_have_the_stable_frontend_json_shape() {
        let value = serde_json::to_value(for_os(PlatformOs::Macos)).unwrap();

        assert_eq!(value["os"], "macos");
        assert_eq!(value["modifier_only_hotkeys"], false);
        assert_eq!(
            value["injection_methods"],
            serde_json::json!(["auto", "clipboard_paste", "clipboard_only"])
        );
    }

    #[test]
    fn unknown_platform_exposes_only_the_non_automated_fallback() {
        let report = for_os(PlatformOs::Other);

        assert!(!report.modifier_only_hotkeys);
        assert_eq!(
            report.injection_methods,
            vec![InjectionCapability::ClipboardOnly]
        );
    }
}
