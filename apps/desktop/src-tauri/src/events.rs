//! Event payload shapes emitted to the UI over Tauri's event bus.
//!
//! Fixed here, once, so every emitter shares the same wire shape rather than
//! each defining its own ad hoc payload.

use std::collections::BTreeMap;

use serde::Serialize;

/// The model-file mutation currently occupying the single operation slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelOperationKind {
    Download,
    Remove,
}

/// User-visible phase of a model-file mutation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelOperationPhase {
    Preparing,
    Downloading,
    Cancelling,
    Removing,
}

/// The active operation. `done`/`total` are bytes for downloads and remain
/// zero for removal or while the server has not reported a content length.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelOperationState {
    pub generation: u64,
    pub id: String,
    pub kind: ModelOperationKind,
    pub phase: ModelOperationPhase,
    pub done: u64,
    pub total: u64,
}

/// Snapshot returned by `model_operation_state` and emitted as
/// `model-operation`. The generation is retained after completion so a late
/// completion event can never clear a newer operation in the frontend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelOperationSnapshot {
    pub generation: u64,
    pub operation: Option<ModelOperationState>,
}

/// The dictation pipeline's current phase, part of the `dictation-state`
/// event payload. Serializes to the lowercase strings the frontend expects
/// (`"idle"`, `"recording"`, ...).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DictationPhase {
    Idle,
    Recording,
    Transcribing,
    Refining,
    Injecting,
}

/// Payload for the `dictation-state` event.
#[derive(Debug, Clone, Serialize)]
pub struct DictationState {
    pub state: DictationPhase,
    pub level: f32,
    pub partial: Option<String>,
}

/// Severity of a `notice` event, shown to the user as a toast.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeKind {
    Info,
    Warning,
    Error,
}

/// Stable meaning of a product-authored notice. The raw English `message`
/// remains on [`Notice`] for logs, compatibility and unknown third-party
/// failures; this code is what lets each presentation layer translate the
/// user-facing wrapper without attempting to translate an arbitrary error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeCode {
    DictationEngineNotRunning,
    NothingHeard,
    RefinementUnavailable,
    AutomaticPasteUnavailable,
    NoLanguageProfile,
    AudioInputUnavailable,
    AudioCaptureFailed,
    TranscriptionStartFailed,
    LivePreviewUnavailable,
    SpeechEngineFailed,
    SpeechEngineFlushFailed,
    HistorySaveFailed,
    ModelDownloadFallback,
    ModelActivationDeferred,
    DictationSetupUnavailable,
    HotkeyUnavailable,
    LivePreviewLimited,
    RefinementApiKeyOptional,
    RefinementSetupUnavailable,
    AutostartSyncFailed,
    SettingsMigrationFailed,
}

/// Payload for the `notice` event.
#[derive(Debug, Clone, Serialize)]
pub struct Notice {
    pub kind: NoticeKind,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<NoticeCode>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub args: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl Notice {
    /// Converts the runtime's legacy text channel into a stable wire payload.
    /// Only strings authored by Utter are recognized. Anything else stays a
    /// raw notice, which is important for OS/provider errors whose wording is
    /// not controlled by this project.
    pub fn from_message(kind: NoticeKind, message: impl Into<String>) -> Self {
        let message = message.into();
        let (code, args, detail) = classify_notice(&message)
            .map(|(code, args, detail)| (Some(code), args, detail))
            .unwrap_or_else(|| (None, BTreeMap::new(), None));
        Self {
            kind,
            message,
            code,
            args,
            detail,
        }
    }

    /// Text for native notifications, which cannot use the webview catalog.
    /// `ru` is explicit; unknown/system preferences safely retain English.
    pub fn localized_message(&self, locale: Option<&str>) -> String {
        let Some(code) = self.code else {
            return self.message.clone();
        };
        let russian = locale
            .and_then(|value| value.split(['-', '_']).next())
            .is_some_and(|root| root.eq_ignore_ascii_case("ru"));
        format_notice(code, &self.args, russian)
    }
}

fn one_arg(name: &str, value: &str) -> BTreeMap<String, String> {
    BTreeMap::from([(name.to_string(), value.to_string())])
}

fn classify_notice(
    message: &str,
) -> Option<(NoticeCode, BTreeMap<String, String>, Option<String>)> {
    let exact = match message {
        "dictation engine is not running" => Some(NoticeCode::DictationEngineNotRunning),
        "Nothing heard" => Some(NoticeCode::NothingHeard),
        "Refinement unavailable — inserted raw transcript" => {
            Some(NoticeCode::RefinementUnavailable)
        }
        "Automatic paste was unavailable, so the text was copied to the clipboard. Check text-injection permission and keep the target field focused." => {
            Some(NoticeCode::AutomaticPasteUnavailable)
        }
        "no language profile is configured; dictation has no hotkey until at least one profile is configured"
        | "no language profiles configured; dictation has no hotkey until at least one profile is configured" => {
            Some(NoticeCode::NoLanguageProfile)
        }
        _ => None,
    };
    if let Some(code) = exact {
        return Some((code, BTreeMap::new(), None));
    }

    if let Some(detail) = message.strip_prefix("could not start capture: ") {
        return Some((
            NoticeCode::AudioCaptureFailed,
            BTreeMap::new(),
            Some(detail.to_string()),
        ));
    }
    if let Some(detail) = message.strip_prefix("failed to start transcription: ") {
        return Some((
            NoticeCode::TranscriptionStartFailed,
            BTreeMap::new(),
            Some(detail.to_string()),
        ));
    }
    if let Some(detail) = message.strip_prefix("speech engine error while flushing: ") {
        return Some((
            NoticeCode::SpeechEngineFlushFailed,
            BTreeMap::new(),
            Some(detail.to_string()),
        ));
    }
    if let Some(detail) = message.strip_prefix("speech engine error: ") {
        return Some((
            NoticeCode::SpeechEngineFailed,
            BTreeMap::new(),
            Some(detail.to_string()),
        ));
    }
    if let Some(detail) = message.strip_prefix("failed to save history entry: ") {
        return Some((
            NoticeCode::HistorySaveFailed,
            BTreeMap::new(),
            Some(detail.to_string()),
        ));
    }
    if let Some(reason) = message
        .strip_prefix("live preview unavailable: ")
        .and_then(|value| {
            value.strip_suffix(". Dictation is unaffected — only the live preview is off.")
        })
    {
        return Some((
            NoticeCode::LivePreviewUnavailable,
            BTreeMap::new(),
            Some(reason.to_string()),
        ));
    }
    if let Some(reason) =
        message.strip_suffix(". Dictation is unaffected — only the live preview is off.")
    {
        return Some((
            NoticeCode::LivePreviewUnavailable,
            BTreeMap::new(),
            Some(reason.to_string()),
        ));
    }
    if let Some(device) = message
        .strip_prefix("Selected audio input \"")
        .and_then(|value| {
            value.strip_suffix(
                "\" is unavailable; using the system default for this run. Your saved device was not changed.",
            )
        })
    {
        return Some((
            NoticeCode::AudioInputUnavailable,
            one_arg("device", device),
            None,
        ));
    }
    if let Some(source) = message
        .strip_prefix(
            "Primary model source is unavailable. Continuing the verified download through ",
        )
        .and_then(|value| value.strip_suffix('.'))
    {
        return Some((
            NoticeCode::ModelDownloadFallback,
            one_arg("source", source),
            None,
        ));
    }
    if let Some(detail) = message
        .strip_prefix("The model was installed, but Utter could not activate it yet: ")
        .and_then(|value| value.strip_suffix(". Save the profile again or restart Utter."))
    {
        return Some((
            NoticeCode::ModelActivationDeferred,
            BTreeMap::new(),
            Some(detail.to_string()),
        ));
    }
    if message.starts_with("profile \"") && message.contains(" has an invalid hotkey ")
        || message.starts_with("failed to start hotkey capture:")
        || message == "hotkey capture is not implemented on this platform yet"
    {
        return Some((
            NoticeCode::HotkeyUnavailable,
            BTreeMap::new(),
            Some(message.to_string()),
        ));
    }
    if message.starts_with("Nemotron preview does not currently use dictionary hotwords")
        || message.starts_with("T-One CTC does not support dictionary hotwords")
    {
        return Some((
            NoticeCode::LivePreviewLimited,
            BTreeMap::new(),
            Some(message.to_string()),
        ));
    }
    if message.starts_with("Refinement is enabled without an API key") {
        return Some((
            NoticeCode::RefinementApiKeyOptional,
            BTreeMap::new(),
            Some(message.to_string()),
        ));
    }
    if message.starts_with("refinement is unavailable:") {
        return Some((
            NoticeCode::RefinementSetupUnavailable,
            BTreeMap::new(),
            Some(message.to_string()),
        ));
    }
    if message.starts_with("Utter saved your Launch at login preference") {
        return Some((
            NoticeCode::AutostartSyncFailed,
            BTreeMap::new(),
            Some(message.to_string()),
        ));
    }
    if message.starts_with("Your settings at ") && message.contains("could not be upgraded") {
        return Some((
            NoticeCode::SettingsMigrationFailed,
            BTreeMap::new(),
            Some(message.to_string()),
        ));
    }
    let setup_issue = message.starts_with("whisper model \"")
        || message.starts_with("sherpa model \"")
        || message.starts_with("failed to load whisper model \"")
        || message.starts_with("failed to load sherpa model \"")
        || message.starts_with("no sherpa model configured")
        || message.starts_with("no cloud speech-to-text API key configured")
        || message.starts_with("this build was compiled without sherpa support")
        || message.starts_with("model \"")
            && (message.contains("model catalog") || message.contains("choose a different model"));
    if setup_issue {
        return Some((
            NoticeCode::DictationSetupUnavailable,
            BTreeMap::new(),
            Some(message.to_string()),
        ));
    }
    None
}

fn format_notice(code: NoticeCode, args: &BTreeMap<String, String>, russian: bool) -> String {
    let device = || args.get("device").map(String::as_str).unwrap_or("—");
    let source = || args.get("source").map(String::as_str).unwrap_or("—");
    match (code, russian) {
        (NoticeCode::DictationEngineNotRunning, false) => "Dictation engine is not running.".into(),
        (NoticeCode::DictationEngineNotRunning, true) => "Движок диктовки не запущен.".into(),
        (NoticeCode::NothingHeard, false) => "Nothing heard.".into(),
        (NoticeCode::NothingHeard, true) => "Речь не распознана.".into(),
        (NoticeCode::RefinementUnavailable, false) => {
            "Refinement is unavailable; the original transcript was inserted.".into()
        }
        (NoticeCode::RefinementUnavailable, true) => {
            "Обработка недоступна — вставлена исходная транскрипция.".into()
        }
        (NoticeCode::AutomaticPasteUnavailable, false) => "Automatic paste was unavailable, so the text was copied to the clipboard. Check text-injection permission and keep the target field focused.".into(),
        (NoticeCode::AutomaticPasteUnavailable, true) => "Автоматическая вставка недоступна, поэтому текст скопирован в буфер обмена. Проверьте разрешение на управление компьютером и оставьте нужное поле в фокусе.".into(),
        (NoticeCode::NoLanguageProfile, false) => "No language profile is configured, so dictation has no hotkey.".into(),
        (NoticeCode::NoLanguageProfile, true) => "Языковой профиль не настроен, поэтому у диктовки нет горячей клавиши.".into(),
        (NoticeCode::AudioInputUnavailable, false) => format!("Audio input \"{}\" is unavailable. The system default is being used for this run.", device()),
        (NoticeCode::AudioInputUnavailable, true) => format!("Аудиовход \"{}\" недоступен. В этом сеансе используется системный микрофон.", device()),
        (NoticeCode::AudioCaptureFailed, false) => "Could not start microphone capture.".into(),
        (NoticeCode::AudioCaptureFailed, true) => "Не удалось начать запись с микрофона.".into(),
        (NoticeCode::TranscriptionStartFailed, false) => "Could not start transcription.".into(),
        (NoticeCode::TranscriptionStartFailed, true) => "Не удалось запустить распознавание речи.".into(),
        (NoticeCode::LivePreviewUnavailable, false) => "Live preview is unavailable. Final dictation is unaffected.".into(),
        (NoticeCode::LivePreviewUnavailable, true) => "Предпросмотр недоступен. Итоговая диктовка продолжит работать.".into(),
        (NoticeCode::SpeechEngineFailed, false) => "The speech engine reported an error.".into(),
        (NoticeCode::SpeechEngineFailed, true) => "Движок распознавания сообщил об ошибке.".into(),
        (NoticeCode::SpeechEngineFlushFailed, false) => "The speech engine could not process the last audio frames.".into(),
        (NoticeCode::SpeechEngineFlushFailed, true) => "Движок распознавания не смог обработать последние фрагменты аудио.".into(),
        (NoticeCode::HistorySaveFailed, false) => "The dictation could not be saved to history.".into(),
        (NoticeCode::HistorySaveFailed, true) => "Не удалось сохранить диктовку в историю.".into(),
        (NoticeCode::ModelDownloadFallback, false) => format!("The primary model source is unavailable. Continuing the verified download through {}.", source()),
        (NoticeCode::ModelDownloadFallback, true) => format!("Основной источник модели недоступен. Проверенная загрузка продолжится через {}.", source()),
        (NoticeCode::ModelActivationDeferred, false) => "The model was installed but could not be activated yet. Save the profile again or restart Utter.".into(),
        (NoticeCode::ModelActivationDeferred, true) => "Модель установлена, но пока не активирована. Сохраните профиль ещё раз или перезапустите Utter.".into(),
        (NoticeCode::DictationSetupUnavailable, false) => "This dictation profile needs attention. Check its engine and model in Settings.".into(),
        (NoticeCode::DictationSetupUnavailable, true) => "Этот профиль диктовки требует настройки. Проверьте его движок и модель в настройках.".into(),
        (NoticeCode::HotkeyUnavailable, false) => "A dictation hotkey is unavailable. Check the profile hotkey and macOS permissions.".into(),
        (NoticeCode::HotkeyUnavailable, true) => "Горячая клавиша диктовки недоступна. Проверьте сочетание в профиле и разрешения macOS.".into(),
        (NoticeCode::LivePreviewLimited, false) => "The selected preview model has limited dictionary support. Final dictation is unaffected.".into(),
        (NoticeCode::LivePreviewLimited, true) => "Выбранная модель предпросмотра ограниченно поддерживает словарь. Итоговая диктовка продолжит работать.".into(),
        (NoticeCode::RefinementApiKeyOptional, false) => "Refinement has no API key. Local providers can work without one; add a key in Settings if your provider requires it.".into(),
        (NoticeCode::RefinementApiKeyOptional, true) => "Для обработки не задан API-ключ. Локальные провайдеры могут работать без него; добавьте ключ, если он нужен вашему провайдеру.".into(),
        (NoticeCode::RefinementSetupUnavailable, false) => "Transcript refinement is unavailable. Check the provider connection and API key in Settings.".into(),
        (NoticeCode::RefinementSetupUnavailable, true) => "Обработка транскрипции недоступна. Проверьте подключение к провайдеру и API-ключ.".into(),
        (NoticeCode::AutostartSyncFailed, false) => "The Launch at login preference was saved, but the operating system could not apply it. Toggle it off and on in Settings.".into(),
        (NoticeCode::AutostartSyncFailed, true) => "Настройка запуска при входе сохранена, но система не смогла её применить. Выключите и снова включите её в настройках.".into(),
        (NoticeCode::SettingsMigrationFailed, false) => "Settings could not be upgraded. Utter is temporarily using defaults; see technical details before changing settings.".into(),
        (NoticeCode::SettingsMigrationFailed, true) => "Не удалось обновить формат настроек. Utter временно использует значения по умолчанию; перед изменениями откройте технические подробности.".into(),
    }
}

#[cfg(test)]
mod notice_tests {
    use super::*;

    #[test]
    fn classifies_product_message_without_putting_the_raw_cause_in_the_template() {
        let notice = Notice::from_message(
            NoticeKind::Error,
            "failed to start transcription: native decoder exploded",
        );
        assert_eq!(notice.code, Some(NoticeCode::TranscriptionStartFailed));
        assert_eq!(notice.detail.as_deref(), Some("native decoder exploded"));
        assert_eq!(
            notice.localized_message(Some("ru-RU")),
            "Не удалось запустить распознавание речи."
        );
        assert_eq!(
            notice.message,
            "failed to start transcription: native decoder exploded"
        );
    }

    #[test]
    fn formats_parameterized_native_notifications_in_both_languages() {
        let notice = Notice::from_message(
            NoticeKind::Warning,
            "Selected audio input \"AirPods\" is unavailable; using the system default for this run. Your saved device was not changed.",
        );
        assert!(notice.localized_message(Some("en")).contains("AirPods"));
        assert!(notice.localized_message(Some("ru")).contains("AirPods"));
        assert!(notice
            .localized_message(Some("ru"))
            .starts_with("Аудиовход"));
    }

    #[test]
    fn leaves_unknown_provider_errors_untouched() {
        let notice = Notice::from_message(NoticeKind::Error, "provider returned HTTP 429");
        assert_eq!(notice.code, None);
        assert_eq!(notice.detail, None);
        assert_eq!(
            notice.localized_message(Some("ru")),
            "provider returned HTTP 429"
        );
    }
}
