//! Language profiles: the unit of configuration a hotkey binding resolves to.
//!
//! Each profile binds one chord to one language and everything that follows
//! from it — which engine transcribes, which model it loads, and whether the
//! transcript is refined afterwards. Pressing a profile's hotkey selects the
//! whole set at once, so the user never picks a language and an engine
//! separately.

use serde::{Deserialize, Serialize};

use utter_core::Tone;

use crate::settings::EngineCfg;

/// One language and everything dictating in it implies.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LanguageProfile {
    /// Stable identifier, used in config, history entries and the HUD. Chosen
    /// by the user rather than generated, so it survives reordering.
    pub id: String,
    /// The chord that selects this profile, in the same syntax as any other
    /// hotkey (`ctrl+alt+super`).
    pub hotkey: String,
    /// BCP-47-style language tag passed to the engine as a transcription hint.
    pub language: String,
    /// Which engine produces the text that gets injected.
    pub engine: EngineCfg,
    /// Which streaming model drives the live preview, if any. `None` — the
    /// default — leaves the preview off; the profile still dictates, and the
    /// text that gets injected always comes from `engine` either way.
    pub draft: Option<DraftCfg>,
    /// How the speech recognizer should be prompted before it decodes this
    /// profile's audio. This is separate from LLM refinement: it biases the
    /// ASR model itself, before a transcript exists.
    pub recognition: RecognitionCfg,
    /// Whether and how this profile's transcripts are refined.
    pub refine: RefinePolicy,
}

impl Default for LanguageProfile {
    fn default() -> Self {
        Self {
            id: "default".to_string(),
            hotkey: "ctrl+super".to_string(),
            language: "en".to_string(),
            engine: EngineCfg::default(),
            draft: None,
            recognition: RecognitionCfg::default(),
            refine: RefinePolicy::default(),
        }
    }
}

/// Per-profile speech-recognition prompt policy.
///
/// `Recommended` selects the prompt recipe associated with the chosen model,
/// `Disabled` suppresses that recipe, and `Custom` uses `custom_prompt`.
/// Dictionary terms remain a separate recognition hint in every mode.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct RecognitionCfg {
    pub prompt_mode: RecognitionPromptMode,
    pub custom_prompt: String,
}

/// Which source supplies the profile's speech-recognition prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RecognitionPromptMode {
    /// Use the tested prompt recipe for the selected model, if one exists.
    #[default]
    Recommended,
    /// Do not add a model recipe; dictionary terms are still supplied.
    Disabled,
    /// Use [`RecognitionCfg::custom_prompt`] instead of a model recipe.
    Custom,
}

/// The streaming model backing a profile's live preview.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct DraftCfg {
    /// Catalog id of the streaming model — never a filesystem path. Resolved
    /// through
    /// [`ModelManager::verify_installed`](crate::ModelManager::verify_installed),
    /// like the sherpa engine's own model id, so a damaged download is caught
    /// before its path reaches the native decoder.
    pub model: String,
}

/// A profile's refinement policy.
///
/// `enabled` here is only half the answer: refinement runs when **both** this
/// flag and the global [`RefineCfg::enabled`](crate::settings::RefineCfg)
/// master switch are set. See
/// [`refinement_is_on`](crate::settings::refinement_is_on), which is the one
/// place that combination is computed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RefinePolicy {
    pub enabled: bool,
    pub tone: Tone,
    /// Optional profile-specific preferences for the LLM editing pass. These
    /// do not affect speech recognition.
    pub instructions: String,
}

impl Default for RefinePolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            tone: Tone::Clean,
            instructions: String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_default_profile_starts_with_refinement_off() {
        // The default engine emits punctuation and casing itself, so there is
        // nothing for a refiner to fix on a fresh install.
        assert!(!RefinePolicy::default().enabled);
        assert!(RefinePolicy::default().instructions.is_empty());
    }

    #[test]
    fn refinement_instructions_round_trip_through_toml() {
        let policy = RefinePolicy {
            enabled: true,
            tone: Tone::Formal,
            instructions: "Prefer em dashes and short paragraphs.".to_string(),
        };
        let text = toml::to_string(&policy).expect("serialize");
        let parsed: RefinePolicy = toml::from_str(&text).expect("deserialize");
        assert_eq!(parsed, policy);
    }

    #[test]
    fn older_refinement_policy_defaults_missing_instructions_to_empty() {
        let policy: RefinePolicy =
            toml::from_str("enabled = true\ntone = \"clean\"").expect("deserialize old policy");
        assert!(policy.instructions.is_empty());
    }

    #[test]
    fn a_draft_config_round_trips_through_toml() {
        let draft = DraftCfg {
            model: "zipformer-ru-small".to_string(),
        };
        let text = toml::to_string(&draft).expect("serialize");
        let parsed: DraftCfg = toml::from_str(&text).expect("deserialize");
        assert_eq!(parsed, draft);
    }

    #[test]
    fn recognition_defaults_to_the_model_recipe() {
        assert_eq!(
            RecognitionCfg::default().prompt_mode,
            RecognitionPromptMode::Recommended
        );
        assert!(RecognitionCfg::default().custom_prompt.is_empty());
    }

    #[test]
    fn recognition_config_round_trips_through_toml() {
        let recognition = RecognitionCfg {
            prompt_mode: RecognitionPromptMode::Custom,
            custom_prompt: "Keep API names in Latin script.".to_string(),
        };
        let text = toml::to_string(&recognition).expect("serialize");
        let parsed: RecognitionCfg = toml::from_str(&text).expect("deserialize");
        assert_eq!(parsed, recognition);
    }
}
