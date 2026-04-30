<script lang="ts">
  import { ChevronDown } from "lucide-svelte";

  import type { FileFilter } from "$lib/types/protocol";
  import { getTreeState, setActiveFilter } from "$lib/stores/fileTree.svelte";

  interface Props {
    filters: FileFilter[];
  }

  let { filters }: Props = $props();
  const state = getTreeState();

  let open = $state(false);

  function pick(filter: FileFilter | null) {
    setActiveFilter(filter);
    open = false;
  }

  let label = $derived(state.activeFilter?.name ?? "All files");

  // Filter init lives in `+page.svelte` because it needs the request's
  // `currentFilter`; doing it here would always default to filters[0]
  // and ignore caller intent (Codex P2). FilterDropdown is purely a
  // controlled view of `state.activeFilter`.
</script>

{#if filters.length > 0}
  <div class="filter">
    <button type="button" class="trigger" onclick={() => (open = !open)}>
      <span>{label}</span>
      <ChevronDown size={12} strokeWidth={2} />
    </button>
    {#if open}
      <ul class="menu" role="listbox">
        {#each filters as filter (filter.name)}
          <li>
            <button
              type="button"
              class="item"
              class:active={state.activeFilter?.name === filter.name}
              onclick={() => pick(filter)}
            >
              {filter.name}
            </button>
          </li>
        {/each}
        <li class="separator" aria-hidden="true"></li>
        <li>
          <button
            type="button"
            class="item"
            class:active={!state.activeFilter}
            onclick={() => pick(null)}
          >
            All files
          </button>
        </li>
      </ul>
    {/if}
  </div>
{/if}

<style>
  .filter {
    position: relative;
  }

  .trigger {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 5px 10px;
    background: transparent;
    color: var(--color-fg-app);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-md);
    font-size: 0.8125rem;
    transition: background 80ms ease;
  }

  .trigger:hover {
    background: color-mix(in srgb, var(--color-fg-app) 6%, transparent);
  }

  .menu {
    position: absolute;
    bottom: calc(100% + 4px);
    left: 0;
    min-width: 180px;
    margin: 0;
    padding: 4px;
    list-style: none;
    background: var(--color-bg-card);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-md);
    box-shadow: var(--shadow-lg);
    z-index: 5;
  }

  .item {
    display: block;
    width: 100%;
    padding: 6px 10px;
    background: transparent;
    color: var(--color-fg-app);
    border: none;
    border-radius: var(--radius-sm);
    text-align: left;
    font-size: 0.8125rem;
  }

  .item:hover {
    background: color-mix(in srgb, var(--color-fg-app) 8%, transparent);
  }

  .item.active {
    background: color-mix(in srgb, var(--color-accent) 25%, transparent);
    color: var(--color-fg-app);
  }

  .separator {
    height: 1px;
    margin: 4px 0;
    background: var(--color-border);
  }
</style>
