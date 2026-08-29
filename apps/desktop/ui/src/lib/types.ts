// TypeScript mirror of `crates/utter-store/src/settings.rs` (and the small
// set of other Rust types the UI talks to). Field names and enum string
// values MUST match the Rust `serde` output exactly (snake_case field names;
// enums use `#[serde(rename_all = "snake_case")]` unless noted) — this file
// is the wire contract with `save_settings`/`get_settings`.

/** `crates/utter-store/src/settings.rs::Theme` */
export type Theme = 'system' | 'light' | 'dark'

/** `crates/utter-core/src/session.rs::DictationMode` */
export type DictationMode = 'push_to_talk' | 'toggle'

/** `crates/utter-store/src/settings.rs::HudPlacement` */
export type HudPlacement = 'auto' | 'pointer' | 'bottom_center'

/** `crates/utter-store/src/settings.rs::EngineKind` */
export type EngineKind = 'whisper' | 'cloud' | 'sherpa'

/** `crates/utter-core/src/types.rs::Tone` */
export type Tone = 'verbatim' | 'clean' | 'formal' | 'notes' | 'code_comment'

/** `crates/utter-store/src/profile.rs::RecognitionPromptMode` */
export type RecognitionPromptMode = 'recommended' | 'disabled' | 'custom'

/** `crates/utter-store/src/settings.rs::InjectionPreference` */
export type InjectionPreference = 'auto' | 'clipboard_paste' | 'type' | 'clipboard_only'

/** `crates/utter-store/src/settings.rs::General` */
export interface General {
  language: string | null
  theme: Theme
  autostart: boolean
}

/** `crates/utter-store/src/settings.rs::Dictation`. The hotkey that triggers
 * dictation lives on each `LanguageProfile` instead — see
 * `LanguageProfile.hotkey`. */
export interface Dictation {
  mode: DictationMode
  silence_timeout_secs: number | null
  hud: boolean
  hud_placement: HudPlacement
}

/** `crates/utter-store/src/settings.rs::CloudSttCfg` */
export interface CloudSttCfg {
  base_url: string
  model: string
}

/** `crates/utter-store/src/settings.rs::EngineCfg` */
export interface EngineCfg {
  active: EngineKind
  whisper_model: string
  sherpa_model: string | null
  cloud: CloudSttCfg
}

/** `crates/utter-store/src/settings.rs::RefineCfg`. Master switch plus the
 * provider connection only — `tone` moved to `RefinePolicy.tone`, set one
 * profile at a time rather than as one global setting. */
export interface RefineCfg {
  enabled: boolean
  base_url: string
  model: string
  timeout_secs: number
}

/** `crates/utter-store/src/profile.rs::RefinePolicy` */
export interface RefinePolicy {
  enabled: boolean
  tone: Tone
  instructions: string
}

/** `crates/utter-store/src/profile.rs::DraftCfg` */
export interface DraftCfg {
  model: string
}

/** `crates/utter-store/src/profile.rs::RecognitionCfg` */
export interface RecognitionCfg {
  prompt_mode: RecognitionPromptMode
  custom_prompt: string
}

/** `crates/utter-store/src/profile.rs::LanguageProfile`. One hotkey chord
 * and everything dictating in it implies: which engine transcribes, which
 * model it loads, and whether the transcript is refined afterwards. */
export interface LanguageProfile {
  id: string
  hotkey: string
  language: string
  engine: EngineCfg
  draft: DraftCfg | null
  recognition: RecognitionCfg
  refine: RefinePolicy
}

/** `crates/utter-refine/src/rules.rs::ReplaceRule` */
export interface ReplaceRule {
  heard: string
  write: string
}

/** `crates/utter-store/src/settings.rs::Dictionary` */
export interface Dictionary {
  terms: string[]
  rules: ReplaceRule[]
}

/** `crates/utter-refine/src/snippets.rs::Snippet` */
export interface Snippet {
  trigger: string
  body: string
}

/** `crates/utter-store/src/settings.rs::HistoryCfg` */
export interface HistoryCfg {
  enabled: boolean
}

/** `crates/utter-store/src/settings.rs::Advanced` */
export interface Advanced {
  injection: InjectionPreference
  audio_device: string | null
  vad_sensitivity: number
  /** Zero means loaded profile models stay resident until reload/quit. */
  model_idle_timeout_secs: number
  log_level: string
}

/** `crates/utter-store/src/settings.rs::Settings` */
export interface Settings {
  general: General
  dictation: Dictation
  refine: RefineCfg
  dictionary: Dictionary
  snippets: Snippet[]
  history: HistoryCfg
  advanced: Advanced
  /** One entry per language the user dictates in, each binding a hotkey to
   * an engine, a model and a refinement policy.
   * `defaultSettings()` below MUST seed exactly the one profile
   * `Settings::default()` does — `App.svelte`'s onboarding gate compares a
   * freshly loaded `Settings` against this function's output key-for-key
   * (see `deepEqual`), so a missing or mismatched profile here makes
   * onboarding never show on a fresh install. */
  profiles: LanguageProfile[]
}

/** Mirrors `Settings::default()` in `crates/utter-store/src/settings.rs`. */
export function defaultSettings(): Settings {
  return {
    general: {
      language: null,
      theme: 'system',
      autostart: false,
    },
    dictation: {
      mode: 'push_to_talk',
      silence_timeout_secs: null,
      hud: true,
      hud_placement: 'auto',
    },
    refine: {
      enabled: false,
      base_url: 'http://localhost:11434/v1',
      model: 'llama3.2',
      timeout_secs: 10,
    },
    dictionary: {
      terms: [],
      rules: [],
    },
    snippets: [],
    history: {
      enabled: true,
    },
    advanced: {
      injection: 'auto',
      audio_device: null,
      vad_sensitivity: 0.5,
      model_idle_timeout_secs: 30 * 60,
      log_level: 'info',
    },
    // Mirrors `Settings::default()`'s single seeded profile exactly: a
    // fresh install gets one profile on the local sherpa-onnx engine (see
    // `EngineCfg::sherpa("parakeet-tdt-110m-en")` in
    // `crates/utter-store/src/settings.rs`), refinement off (the sherpa
    // models already emit punctuation and casing), and no draft/streaming
    // model configured yet.
    profiles: [
      {
        id: 'default',
        hotkey: 'ctrl+super',
        language: 'en',
        engine: {
          active: 'sherpa',
          whisper_model: 'small',
          sherpa_model: 'parakeet-tdt-110m-en',
          cloud: {
            base_url: 'https://api.openai.com/v1',
            model: 'whisper-1',
          },
        },
        draft: null,
        recognition: {
          prompt_mode: 'recommended',
          custom_prompt: '',
        },
        refine: {
          enabled: false,
          tone: 'clean',
          instructions: '',
        },
      },
    ],
  }
}

/** Structural deep equality: object key order never matters (unlike a naive
 * `JSON.stringify` comparison, which is sensitive to it), arrays compare
 * element-by-element in order, and primitives compare with `===`. Sufficient
 * for comparing two `Settings` values without depending on both having been
 * serialized with identical key ordering. */
export function deepEqual(a: unknown, b: unknown): boolean {
  if (a === b) return true
  if (typeof a !== typeof b || a === null || b === null) return false

  if (Array.isArray(a) || Array.isArray(b)) {
    if (!Array.isArray(a) || !Array.isArray(b) || a.length !== b.length) return false
    return a.every((value, i) => deepEqual(value, b[i]))
  }

  if (typeof a === 'object' && typeof b === 'object') {
    const aRecord = a as Record<string, unknown>
    const bRecord = b as Record<string, unknown>
    const aKeys = Object.keys(aRecord)
    const bKeys = Object.keys(bRecord)
    if (aKeys.length !== bKeys.length) return false
    return aKeys.every(
      (key) =>
        Object.prototype.hasOwnProperty.call(bRecord, key) &&
        deepEqual(aRecord[key], bRecord[key]),
    )
  }

  return false
}

/** `crates/utter-store/src/models.rs::ModelInfo` */
export type ModelRole = 'final' | 'preview'
export type PerformanceClass = 'fast' | 'balanced' | 'heavy'
export type ModelDownloadOutcome = 'installed' | 'cancelled'
export type ModelInstallStatus = 'missing' | 'ready' | 'damaged'

export interface ModelInfo {
  id: string
  engine: string
  label: string
  size_mb: number
  installed: boolean
  status: ModelInstallStatus
  /** BCP-47 prefixes; `*` means broadly multilingual. */
  supported_languages: string[]
  role: ModelRole
  performance_class: PerformanceClass
  recommendation_tags: string[]
}

/** `crates/utter-store/src/history.rs::HistoryEntry` */
export interface HistoryEntry {
  id: number
  created_at: number
  duration_ms: number
  engine: string
  raw_text: string
  final_text: string
  app: string | null
}

/** `src-tauri/src/permissions.rs::PermissionReport` */
export type PermissionReport =
  | {
      platform: 'linux'
      input_group: boolean
      uinput_writable: boolean
      fix_command: string
    }
  | {
      platform: 'macos'
      microphone: PermissionStatus
      text_injection: PermissionStatus
      bundle_id: string
      microphone_reset_command: string
      text_injection_reset_command: string
    }
  | {
      platform: 'unsupported'
      os: string
    }

export type PermissionStatus = 'not_determined' | 'granted' | 'denied' | 'unavailable'
export type PermissionKind = 'microphone' | 'text_injection'

/** `src-tauri/src/platform.rs::PlatformCapabilities` */
export interface PlatformCapabilities {
  os: 'linux' | 'macos' | 'other'
  modifier_only_hotkeys: boolean
  injection_methods: InjectionPreference[]
  updater: boolean
}

/** Release-only updater command/event contracts from `src-tauri/src/updater.rs`. */
export interface UpdateInfo {
  version: string
  notes: string | null
}

export interface UpdateCheck {
  current_version: string
  update: UpdateInfo | null
}

export type UpdateProgressPayload =
  | { event: 'started'; total: number | null }
  | { event: 'progress'; downloaded: number; total: number | null }
  | { event: 'finished' }

/** `src-tauri/src/events.rs::ModelOperationKind` */
export type ModelOperationKind = 'download' | 'remove'

/** `src-tauri/src/events.rs::ModelOperationPhase` */
export type ModelOperationPhase = 'preparing' | 'downloading' | 'cancelling' | 'removing'

/** The active model mutation (`src-tauri/src/events.rs::ModelOperationState`). */
export interface ModelOperationState {
  generation: number
  id: string
  kind: ModelOperationKind
  phase: ModelOperationPhase
  done: number
  total: number
}

/** Command/event snapshot. `operation` is null after this generation ended. */
export interface ModelOperationSnapshot {
  generation: number
  operation: ModelOperationState | null
}

/** `src-tauri/src/events.rs::DictationPhase` */
export type DictationPhase = 'idle' | 'recording' | 'transcribing' | 'refining' | 'injecting'

/** Payload of the `dictation-state` event (`src-tauri/src/events.rs::DictationState`). */
export interface DictationStatePayload {
  state: DictationPhase
  level: number
  partial: string | null
}

/** `src-tauri/src/events.rs::NoticeKind` */
export type NoticeKind = 'info' | 'warning' | 'error'

/** Payload of the `notice` event (`src-tauri/src/events.rs::Notice`). */
export interface NoticePayload {
  kind: NoticeKind
  message: string
}

/** The two api-key "service" identities `set_api_key`/`has_api_key` accept
 * (`STT_KEY_SERVICE` / `REFINE_KEY_SERVICE` in `src-tauri/src/lib.rs`). */
export type ApiKeyService = 'stt' | 'refine'
