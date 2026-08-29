//! Event payload shapes emitted to the UI over Tauri's event bus.
//!
//! Fixed here, once, so every emitter shares the same wire shape rather than
//! each defining its own ad hoc payload.

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

/// Payload for the `notice` event.
#[derive(Debug, Clone, Serialize)]
pub struct Notice {
    pub kind: NoticeKind,
    pub message: String,
}
