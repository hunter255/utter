//! Settings, history and model storage.

pub mod error;
pub mod history;
pub mod identity;
pub mod migrate;
pub mod models;
pub mod profile;
pub mod settings;

pub use error::{IntegrityError, MigrateError, MigrationFailed};
pub use history::{HistoryEntry, HistoryRepo, NewEntry};
pub use identity::{
    config_path, data_dir, migrate_legacy_storage, StorageMigrationReport, APP_IDENTIFIER,
    LEGACY_STORAGE_IDENTIFIER,
};
pub use migrate::migrate_v1;
pub use models::{
    DownloadCancellation, DownloadCancelled, DownloadStalled, ModelInfo, ModelManager, ModelRole,
    PerformanceClass, StreamingModelFamily,
};
pub use profile::{DraftCfg, LanguageProfile, RecognitionCfg, RecognitionPromptMode, RefinePolicy};
pub use settings::{load, save, HudPlacement, Settings};
