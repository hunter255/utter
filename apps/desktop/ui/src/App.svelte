<script lang="ts">
  import { onDestroy, onMount } from 'svelte'
  import type { UnlistenFn } from '@tauri-apps/api/event'

  import Notices from './lib/components/Notices.svelte'
  import ModelOperationStatus from './lib/components/ModelOperationStatus.svelte'
  import * as api from './lib/api'
  import { modelStore } from './lib/model-store'
  import { noticeStore } from './lib/notices'
  import { t } from './lib/i18n'
  import { broadcastLocalePreference } from './lib/locale-sync'
  import {
    SETTINGS_NAV,
    resolveSettingsSection,
    settingsWindowTitle,
    type SettingsSection,
  } from './lib/settings-nav'
  import { settingsStore } from './lib/stores'
  import { applyTheme } from './lib/theme'
  import { deepEqual, defaultSettings, type PlatformCapabilities } from './lib/types'

  import Profiles from './pages/Profiles.svelte'
  import Models from './pages/Engines.svelte'
  import Connections from './pages/Refinement.svelte'
  import Vocabulary from './pages/Vocabulary.svelte'
  import History from './pages/History.svelte'
  import Settings from './pages/Settings.svelte'
  import Onboarding from './pages/Onboarding.svelte'

  // The onboarding wizard is shown when we have no signal that this user has
  // ever been through it or configured anything: no completion flag in
  // localStorage (nothing else survives an uninstall/reinstall or a "start
  // fresh" without the flag), AND the loaded settings are byte-for-byte the
  // defaults (a real config file that merely doesn't set some fields would
  // still differ once anything meaningful was changed and saved). This is a
  // heuristic, not a hard signal — a new Rust command (e.g. "does
  // config.toml exist") was deliberately ruled out for this, so there is no
  // way to distinguish "never configured" from "configured everything back
  // to the exact defaults on purpose" without one. A dedicated
  // `config_exists` command would remove this ambiguity later.
  const ONBOARDED_KEY = 'utter.onboarded'
  const LAST_SECTION_KEY = 'utter.settings.lastSection'

  function currentHash(): SettingsSection {
    const raw = window.location.hash.replace(/^#/, '')
    const section = resolveSettingsSection(raw, localStorage.getItem(LAST_SECTION_KEY) ?? '')
    if (window.location.hash !== `#${section}`) {
      // Hash links are already the app's navigation primitive. Direct hash
      // assignment also works under Tauri's custom protocol, while WebKit
      // can reject `history.replaceState` for that protocol before Svelte
      // has mounted and leave a blank window.
      window.location.hash = section
    }
    return section
  }

  let hash = $state(currentHash())
  let unlistenNotices: UnlistenFn | undefined
  let loading = $state(true)
  let loadError = $state('')
  let showOnboarding = $state(false)
  let capabilities = $state<PlatformCapabilities | null>(null)

  function isDefaultSettings(settings: unknown): boolean {
    return deepEqual(settings, defaultSettings())
  }

  function finishOnboarding() {
    localStorage.setItem(ONBOARDED_KEY, '1')
    showOnboarding = false
  }

  function onHashChange() {
    void settingsStore.flush()
    hash = currentHash()
  }

  function onBeforeUnload() {
    void settingsStore.flush()
  }

  onMount(async () => {
    // Subscribed before settings are loaded, and outside the try: a notice
    // is most worth reading when something else went wrong, including the
    // load below.
    noticeStore
      .start()
      .then((fn) => {
        unlistenNotices = fn
      })
      .catch(() => {})

    try {
      const [loaded, loadedCapabilities] = await Promise.all([
        settingsStore.load(),
        api.platformCapabilities(),
        modelStore.start(),
      ])
      void broadcastLocalePreference(loaded.general.language ?? 'system')
      capabilities = loadedCapabilities
      showOnboarding = !localStorage.getItem(ONBOARDED_KEY) && isDefaultSettings(loaded)
    } catch (err) {
      loadError = String(err)
    } finally {
      loading = false
    }

    window.addEventListener('hashchange', onHashChange)
    window.addEventListener('beforeunload', onBeforeUnload)
  })

  onDestroy(() => {
    window.removeEventListener('hashchange', onHashChange)
    window.removeEventListener('beforeunload', onBeforeUnload)
    unlistenNotices?.()
    modelStore.stop()
    // A patch made right before this window closes (e.g. the user tweaked a
    // field and immediately hit the OS close button) must not be dropped
    // just because the 500ms debounce hadn't elapsed yet.
    void settingsStore.flush()
  })

  $effect(() => {
    if ($settingsStore) applyTheme($settingsStore.general.theme)
  })

  $effect(() => {
    const title = settingsWindowTitle(hash, $t)
    localStorage.setItem(LAST_SECTION_KEY, hash)
    document.title = title
    void api.setWindowTitle(title).catch(() => {})
  })
</script>

{#if loading}
  <div class="status">{$t('app.loadingSettings')}</div>
{:else if loadError}
  <div class="status error">{$t('app.loadSettingsFailed', { error: loadError })}</div>
{:else if showOnboarding && capabilities}
  <Onboarding onDone={finishOnboarding} {capabilities} />
{:else if $settingsStore && capabilities}
  <div class="shell">
    <nav aria-label={$t('app.settingsSections')}>
      <div class="brand">Utter</div>
      {#each SETTINGS_NAV as group (group.labelKey)}
        <section class="nav-group" aria-label={$t(group.labelKey)}>
          <div class="nav-group-label">{$t(group.labelKey)}</div>
          <ul>
            {#each group.items as item (item.hash)}
              <li>
                <a
                  href="#{item.hash}"
                  aria-current={hash === item.hash ? 'page' : undefined}
                  class:active={hash === item.hash}
                >
                  {$t(item.labelKey)}
                </a>
              </li>
            {/each}
          </ul>
        </section>
      {/each}
    </nav>
    <main>
      {#if hash === 'profiles'}
        <Profiles {capabilities} />
      {:else if hash === 'models'}
        <Models />
      {:else if hash === 'connections'}
        <Connections />
      {:else if hash === 'vocabulary'}
        <Vocabulary />
      {:else if hash === 'history'}
        <History />
      {:else if hash === 'settings'}
        <Settings {capabilities} />
      {/if}
    </main>
  </div>
{/if}

<!-- Model operations outlive every route component, so their progress stays
     visible while the user moves between settings sections or onboarding. -->
<ModelOperationStatus />

<!-- Outside the branches above: a notice has to be readable whichever of
     them is on screen, including the settings-load error. -->
<Notices />

<style>
  .status {
    display: flex;
    align-items: center;
    justify-content: center;
    height: 100vh;
    color: var(--text-muted);
    font-size: 14px;
  }

  .status.error {
    color: var(--danger);
  }

  .shell {
    display: grid;
    grid-template-columns: 200px 1fr;
    height: 100vh;
  }

  nav {
    background: var(--bg-sunken);
    border-right: 1px solid var(--border);
    padding: var(--space-4) var(--space-2);
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
  }

  .brand {
    font-weight: 700;
    font-size: 15px;
    padding: 0 var(--space-2);
  }

  .nav-group {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }

  .nav-group-label {
    padding: 0 var(--space-2);
    color: var(--text-muted);
    font-size: 10px;
    font-weight: 700;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }

  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  a {
    display: block;
    padding: var(--space-2);
    border-radius: var(--radius-sm);
    color: var(--text);
    text-decoration: none;
    font-size: 13px;
    font-weight: 500;
  }

  a:hover {
    background: var(--bg-elevated);
  }

  a:active:not(.active) {
    background: var(--surface-hover);
  }

  a.active {
    background: var(--accent);
    color: var(--accent-contrast);
  }

  main {
    overflow-y: auto;
    padding: var(--space-6);
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
    max-width: 760px;
  }
</style>
