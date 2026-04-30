<script lang="ts">
  import { getTreeState, setSaveFilename, validateFilename } from "$lib/stores/fileTree.svelte";

  const state = getTreeState();

  function onInput(e: Event) {
    const target = e.target as HTMLInputElement;
    setSaveFilename(target.value);
  }

  let validationError = $derived(validateFilename(state.saveFilename));
</script>

<div class="save-bar">
  <label>
    <span class="label">Save as</span>
    <input
      type="text"
      placeholder="filename"
      value={state.saveFilename}
      oninput={onInput}
      autocomplete="off"
      spellcheck="false"
      aria-invalid={validationError !== null}
    />
  </label>
  {#if validationError && state.saveFilename.length > 0}
    <p class="error">{validationError}</p>
  {/if}
</div>

<style>
  .save-bar {
    padding: 8px 16px;
    border-top: 1px solid var(--color-border);
    background: var(--color-bg-card);
  }

  label {
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .label {
    font-size: 0.8125rem;
    color: var(--color-fg-muted);
    flex-shrink: 0;
  }

  input {
    flex: 1;
    padding: 5px 10px;
    background: var(--color-bg-app);
    color: var(--color-fg-app);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    font-size: 0.875rem;
    outline: none;
    font-family: ui-monospace, SFMono-Regular, monospace;
  }

  input:focus {
    border-color: color-mix(in srgb, var(--color-accent) 60%, transparent);
  }

  input[aria-invalid="true"] {
    border-color: var(--color-danger);
  }

  .error {
    margin: 4px 0 0;
    font-size: 0.75rem;
    color: var(--color-danger);
  }
</style>
