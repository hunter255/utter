//! Resolves a language profile's speech-recognition prompt.
//!
//! The result is passed to the STT adapter before decoding. It is deliberately
//! separate from `utter-refine`: LLM refinement sees an already-produced
//! transcript, while this prompt changes what the recognizer produces in the
//! first place.

use utter_store::settings::EngineKind;
use utter_store::{LanguageProfile, RecognitionPromptMode};

// Short model-specific prompts derived from the benchmark recipes. Keeping
// them here, at the product wiring layer, leaves `utter-stt` model-agnostic:
// the adapter only knows that it received an initial prompt, not which catalog
// id caused the app to choose it.
const PUNCTUATED_RUSSIAN: &str = "Привет! Как дела? Он ответил: «Сделаем это сегодня — времени достаточно». Конечно, всё не так просто; сначала нужно проверить результат.";
const BILINGUAL_INSTRUCTION: &str = "Transcribe bilingual Russian-English speech. Keep embedded English IT terms in Latin script: Claude Code, GitHub, feature branch, CI/CD pipeline, deployment.";
const BILINGUAL_EXAMPLE: &str = "This recording contains Russian and English speech. Preserve English technical terms in Latin script. Example: «Мы задеплоили feature в production через CI/CD pipeline в Claude Code».";

/// Produces the complete per-utterance prompt: the selected model recipe (or
/// custom prompt), followed by the user's preferred dictionary spellings.
///
/// Dictionary terms are independent of `prompt_mode`. Disabling a punctuation
/// recipe must not silently disable the Dictionary feature, and placing terms
/// last gives user-specific spellings priority if whisper.cpp has to trim an
/// unusually long initial prompt to its decoder context.
pub(crate) fn initial_prompt_for(
    profile: &LanguageProfile,
    dictionary_terms: &[String],
) -> Option<String> {
    let base = match profile.recognition.prompt_mode {
        RecognitionPromptMode::Recommended => recommended_prompt(profile),
        RecognitionPromptMode::Disabled => None,
        RecognitionPromptMode::Custom => nonblank(&profile.recognition.custom_prompt),
    };

    let dictionary = dictionary_terms
        .iter()
        .map(|term| term.trim())
        .filter(|term| !term.is_empty())
        .collect::<Vec<_>>()
        .join(", ");

    [base, nonblank(&dictionary)]
        .into_iter()
        .flatten()
        .map(str::to_string)
        .reduce(|mut prompt, part| {
            prompt.push_str("\n\n");
            prompt.push_str(&part);
            prompt
        })
}

fn recommended_prompt(profile: &LanguageProfile) -> Option<&'static str> {
    if profile.engine.active != EngineKind::Whisper {
        return None;
    }

    match profile.engine.whisper_model.as_str() {
        // The benchmark's instruction-plus-example recipe is critical for
        // Turbo punctuation and also wins for full-precision Medium.
        "large-v3-turbo-q5_0" | "medium" => Some(BILINGUAL_EXAMPLE),
        // Breeze responds best to a direct bilingual instruction.
        "breeze-asr-25-q5_k" => Some(BILINGUAL_INSTRUCTION),
        // Stable Large v2 needs only a natural punctuation seed.
        "large-v2-q5_0" => Some(PUNCTUATED_RUSSIAN),
        _ => None,
    }
}

fn nonblank(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

#[cfg(test)]
mod tests {
    use utter_store::settings::EngineCfg;
    use utter_store::{RecognitionCfg, RecognitionPromptMode};

    use super::*;

    fn whisper_profile(model: &str) -> LanguageProfile {
        LanguageProfile {
            engine: EngineCfg {
                whisper_model: model.to_string(),
                ..EngineCfg::default()
            },
            ..LanguageProfile::default()
        }
    }

    #[test]
    fn recommended_turbo_prompt_precedes_dictionary_terms() {
        let profile = whisper_profile("large-v3-turbo-q5_0");
        let prompt = initial_prompt_for(
            &profile,
            &[" PostgreSQL ".to_string(), "Claude Code".to_string()],
        )
        .expect("prompt");

        assert!(prompt.starts_with(BILINGUAL_EXAMPLE));
        assert!(prompt.ends_with("PostgreSQL, Claude Code"));
    }

    #[test]
    fn every_benchmarked_model_gets_its_own_recipe() {
        assert_eq!(
            recommended_prompt(&whisper_profile("medium")),
            Some(BILINGUAL_EXAMPLE)
        );
        assert_eq!(
            recommended_prompt(&whisper_profile("breeze-asr-25-q5_k")),
            Some(BILINGUAL_INSTRUCTION)
        );
        assert_eq!(
            recommended_prompt(&whisper_profile("large-v2-q5_0")),
            Some(PUNCTUATED_RUSSIAN)
        );
        assert_eq!(recommended_prompt(&whisper_profile("small")), None);
    }

    #[test]
    fn disabled_recipe_keeps_dictionary_biasing() {
        let mut profile = whisper_profile("large-v3-turbo-q5_0");
        profile.recognition.prompt_mode = RecognitionPromptMode::Disabled;
        assert_eq!(
            initial_prompt_for(&profile, &["SwiftUI".to_string()]).as_deref(),
            Some("SwiftUI")
        );
    }

    #[test]
    fn custom_prompt_replaces_recipe_and_ignores_surrounding_whitespace() {
        let mut profile = whisper_profile("large-v3-turbo-q5_0");
        profile.recognition = RecognitionCfg {
            prompt_mode: RecognitionPromptMode::Custom,
            custom_prompt: "  Preserve product names.  ".to_string(),
        };
        assert_eq!(
            initial_prompt_for(&profile, &[]).as_deref(),
            Some("Preserve product names.")
        );
    }

    #[test]
    fn recommended_non_whisper_profile_only_uses_dictionary() {
        let profile = LanguageProfile {
            engine: EngineCfg::sherpa("gigaam-v3-e2e-rnnt"),
            ..LanguageProfile::default()
        };
        assert_eq!(
            initial_prompt_for(&profile, &["GigaAM".to_string()]).as_deref(),
            Some("GigaAM")
        );
    }

    #[test]
    fn no_recipe_and_no_dictionary_produces_no_prompt() {
        assert_eq!(initial_prompt_for(&whisper_profile("small"), &[]), None);
    }
}
