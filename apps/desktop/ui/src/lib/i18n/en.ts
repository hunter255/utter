/** English is the source catalog: every translated catalog must use these keys. */
export const en = {
  'app.loadingSettings': 'Loading settings…',
  'app.loadSettingsFailed': 'Failed to load settings: {error}',
  'app.settingsSections': 'Settings sections',
  'app.windowTitle': '{section} — Utter',

  'nav.group.dictation': 'Dictation',
  'nav.group.resources': 'Resources',
  'nav.group.application': 'Application',
  'nav.profiles': 'Profiles',
  'nav.vocabulary': 'Vocabulary',
  'nav.history': 'History',
  'nav.models': 'Models',
  'nav.connections': 'Connections',
  'nav.settings': 'Settings',

  'common.allow': 'Allow',
  'common.back': 'Back',
  'common.cancel': 'Cancel',
  'common.cancelling': 'Cancelling…',
  'common.checkAgain': 'Check again',
  'common.checking': 'Checking…',
  'common.continue': 'Continue',
  'common.copied': 'Copied',
  'common.finish': 'Finish',
  'common.needsSetup': 'Needs setup',
  'common.ready': 'Ready',
  'common.requesting': 'Requesting…',
  'common.skipSetup': 'Skip setup',

  'hotkey.pressKeys': 'Press keys…',
  'hotkey.pressKeysHint': 'Press keys… (Esc to cancel)',
  'hotkey.clickToSet': 'Click to set…',
  'hotkey.addBaseKey': 'Add a letter, number, function key, Space, `, or Insert',
  'hotkey.oneBaseKey': 'A hotkey may only have one base key',
  'hotkey.releaseToConfirm': 'Release all keys to confirm',

  'permission.openSettings': 'Open settings',
  'permission.recovery': 'Permission recovery',
  'permission.recoveryHint':
    'Use this only when Utter is missing from System Settings or its saved permission is stale.',
  'permission.copyResetCommand': 'Copy reset command',
  'permission.status.granted': 'granted',
  'permission.status.denied': 'denied',
  'permission.status.notDetermined': 'not determined',
  'permission.status.unavailable': 'unavailable',

  'model.readyToUse': 'Ready to use',
  'model.preparingDownload': 'Preparing download…',
  'model.downloadingPercent': 'Downloading {percent}%',
  'model.filesDamaged': 'Model files are damaged',
  'model.selectedNotInstalled': 'Selected, not installed',
  'model.redownloadSize': 'Re-download {size} MB',
  'model.downloadSizeAndUse': 'Download {size} MB and use',
  'model.downloadAria': 'Downloading {model}',
  'model.operation': 'Model operation',
  'model.removing': 'Removing…',

  'notice.info': 'Notice',
  'notice.warning': 'Warning',
  'notice.error': 'Error',
  'notice.dismiss': 'Dismiss notice',

  'hud.state.idle': 'Idle',
  'hud.state.recording': 'Listening',
  'hud.state.transcribing': 'Transcribing',
  'hud.state.refining': 'Refining',
  'hud.state.injecting': 'Injecting',
  'hud.signal.none': 'No signal',
  'hud.signal.quiet': 'Quiet signal',
  'hud.signal.voice': 'Voice detected',
  'hud.preview.recording': 'Listening for speech…',
  'hud.preview.transcribing': 'Preparing the final transcript…',
  'hud.preview.refining': 'Refining the transcript…',
  'hud.preview.injecting': 'Delivering text…',
  'hud.microphoneLevel': 'Microphone input level',

  'onboarding.step.welcome': 'Welcome',
  'onboarding.step.microphone': 'Microphone',
  'onboarding.step.model': 'Model',
  'onboarding.step.hotkey': 'Hotkey',
  'onboarding.step.permissions': 'Permissions',
  'onboarding.step.done': 'Done',
  'onboarding.welcomeTitle': 'Welcome to Utter',
  'onboarding.welcomeBody':
    'A quick, skippable setup: microphone, a speech model, your hotkey, and permissions.',
  'onboarding.microphoneTitle': 'Microphone',
  'onboarding.microphoneNeedsAccess':
    'Utter needs microphone access to record speech for transcription.',
  'onboarding.allowMicrophone': 'Allow microphone',
  'onboarding.microphoneOff':
    'Microphone access is off. Enable Utter in System Settings → Privacy & Security → Microphone, then return here.',
  'onboarding.microphoneRecoveryHint':
    'If Utter is missing or the status is stale, copy the command, quit Utter, run it in Terminal, then reopen the app and allow access again.',
  'onboarding.microphoneUnavailable': 'Microphone permission is unavailable on this Mac.',
  'onboarding.microphoneCheckHint':
    'This confirms your system reports an input device; live recording is tested when you dictate.',
  'onboarding.noInputDevices': 'No input devices were found. Check your microphone connection.',
  'onboarding.foundInputDevice': 'Found {count} input device:',
  'onboarding.foundInputDevices': 'Found {count} input devices:',
  'onboarding.modelTitle': 'Speech model',
  'onboarding.modelBody':
    'Choose the language and local model for your first profile. All final-transcript models in the catalog are available here; live-preview-only models stay separate.',
  'onboarding.language': 'Language',
  'onboarding.model': 'Model',
  'onboarding.loadingModels': 'Loading models…',
  'onboarding.chooseLocalModel': 'Choose a local model',
  'onboarding.cloudProfile':
    'Your default profile dictates through a cloud speech-to-text endpoint. Configure its API key under Connections after finishing setup, or choose a local model above.',
  'onboarding.modelInstalled': "This model is installed — you're ready to dictate.",
  'onboarding.chooseModelForLocal':
    'Choose a model before continuing if you want to dictate locally.',
  'onboarding.hotkeyTitle': 'Hotkey',
  'onboarding.hotkeyBody': 'Pick the key combination that starts/stops dictation.',
  'onboarding.macosNeedsBaseKey':
    'macOS needs a base key; modifiers are optional. Try `, Insert, F5, or ctrl+alt+space.',
  'onboarding.chooseHotkey': 'Choose a hotkey before continuing.',
  'onboarding.permissionsTitle': 'Permissions',
  'onboarding.linuxPermissionsIntro':
    'Linux hotkeys and text injection need two OS-level permissions.',
  'onboarding.inputGroup': 'Input device group membership',
  'onboarding.uinputWritable': '/dev/uinput writable',
  'onboarding.copyFixCommand': 'Copy fix command',
  'onboarding.allPermissionsAlreadyGranted': 'All required permissions are already granted.',
  'onboarding.permissionRequestHint':
    'These permissions are requested only when you press an Allow button. You can continue with reduced functionality if either remains off.',
  'onboarding.microphonePermission': 'Microphone — {status}',
  'onboarding.pastePermission': 'Paste and caret-relative HUD — {status}',
  'onboarding.deniedPermissionHint':
    'Enable the denied access in System Settings → Privacy & Security, then return to Utter and check again. Dictation needs Microphone; automatic paste and precise HUD position need Accessibility.',
  'onboarding.microphoneRecovery': 'Microphone recovery',
  'onboarding.accessibilityRecovery': 'Accessibility recovery',
  'onboarding.resetPermissionHint':
    'Use reset only if the System Settings entry is missing or stale: copy the command, quit Utter, run it in Terminal, then reopen Utter and allow access again.',
  'onboarding.allPermissionsGranted': 'All required permissions are granted.',
  'onboarding.permissionsUnsupported':
    'Permission setup for {os} is not available in this build yet. You can continue and configure platform access later.',
  'onboarding.doneTitle': "You're all set",
  'onboarding.doneBody': 'You can revisit any of this later from the settings sidebar.',
} as const

export type EnglishCatalog = typeof en
export type MessageKey = keyof EnglishCatalog
