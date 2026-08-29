<script lang="ts">
  import DictionaryPage from './Dictionary.svelte'
  import Snippets from './Snippets.svelte'
</script>

<header class="page-heading">
  <h1>Vocabulary</h1>
  <p>Teach recognition special words, correct recurring mistakes, and expand spoken commands.</p>
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
