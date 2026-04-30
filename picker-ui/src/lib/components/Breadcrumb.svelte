<script lang="ts">
  import { ChevronRight, Home } from "lucide-svelte";

  interface Props {
    path: string;
    onNavigate: (segment: string) => void;
  }

  let { path, onNavigate }: Props = $props();

  /// Build clickable segments. `/home/user/Documents/Projects` becomes
  /// `[Home, user, Documents, Projects]` where each segment carries
  /// its absolute path so `onNavigate` can `cd` to it directly.
  function segments(p: string): { label: string; absolute: string; isHome: boolean }[] {
    if (!p) return [];
    const out = [];
    const parts = p.split("/").filter((s) => s.length > 0);
    let acc = "";
    out.push({ label: "/", absolute: "/", isHome: false });
    for (const part of parts) {
      acc = `${acc}/${part}`;
      out.push({ label: part, absolute: acc, isHome: false });
    }
    return out;
  }

  let segs = $derived(segments(path));
</script>

<nav class="breadcrumb">
  {#each segs as seg, i (seg.absolute)}
    {#if i > 0}
      <ChevronRight size={12} strokeWidth={2} class="sep" />
    {/if}
    <button
      type="button"
      class="seg"
      class:current={i === segs.length - 1}
      onclick={() => onNavigate(seg.absolute)}
      title={seg.absolute}
    >
      {#if i === 0}
        <Home size={12} strokeWidth={2} />
      {:else}
        {seg.label}
      {/if}
    </button>
  {/each}
</nav>

<style>
  .breadcrumb {
    display: flex;
    align-items: center;
    gap: 4px;
    overflow-x: auto;
    overflow-y: hidden;
    padding: 4px 0;
    scrollbar-width: thin;
  }

  .seg {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 3px 8px;
    border: none;
    background: transparent;
    color: var(--color-fg-muted);
    border-radius: var(--radius-sm);
    font-size: 0.8125rem;
    transition: background 80ms ease, color 80ms ease;
    white-space: nowrap;
  }

  .seg:hover {
    background: color-mix(in srgb, var(--color-fg-app) 8%, transparent);
    color: var(--color-fg-app);
  }

  .seg.current {
    color: var(--color-fg-app);
    font-weight: 500;
  }

  :global(.breadcrumb .sep) {
    color: color-mix(in srgb, var(--color-fg-app) 30%, transparent);
    flex-shrink: 0;
  }
</style>
