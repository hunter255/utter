<script lang="ts">
  import { t } from '../lib/i18n'
  import DictionaryPage from './Dictionary.svelte'
  import Snippets from './Snippets.svelte'
</script>

<header class="page-heading">
  <h1>{$t('vocabulary.title')}</h1>
  <p>{$t('vocabulary.description')}</p>
</header>

<DictionaryPage />
<Snippets />

<style>
  .page-heading {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }

  h1 {
    font-size: 20px;
    font-weight: 700;
  }

  p {
    color: var(--text-muted);
    font-size: 13px;
  }
</style>
