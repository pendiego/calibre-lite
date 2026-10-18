<script lang="ts">
  import { onMount } from 'svelte';
  import { searchMetadata, applyMetadata, type BookRow, type Candidate, type SearchResult } from './api';

  let { book, onclose }: { book: BookRow; onclose: (changed: boolean) => void } = $props();

  let result = $state<SearchResult | null>(null);
  let selected = $state<number | null>(null);
  let useMeta = $state(true);
  let useCover = $state(true);
  let error = $state('');
  let busy = $state(false);

  const label = { google: 'Google Books', openlibrary: 'Open Library' };
  const chosen = $derived<Candidate | null>(selected === null || !result ? null : result.candidates[selected]);

  onMount(async () => {
    try {
      result = await searchMetadata(book.id);
    } catch (e) {
      result = { candidates: [], errors: [String(e)] };
    }
  });

  async function apply() {
    if (!chosen) return;
    busy = true;
    error = '';
    try {
      await applyMetadata(book.id, chosen, useMeta, useCover && !!chosen.coverUrl);
      onclose(true);
    } catch (e) {
      error = String(e);
      busy = false;
    }
  }
</script>

<div class="backdrop">
  <div class="dialog">
    <h2>Baixar metadados: {book.title}</h2>
    <div class="list">
      {#if !result}
        <p class="muted">Buscando…</p>
      {:else}
        {#each result.candidates as c, i}
          <label class="cand" class:sel={selected === i}>
            <input type="radio" name="cand" value={i} bind:group={selected} />
            {#if c.coverUrl}<img src={c.coverUrl} alt="" width="60" loading="lazy" />{:else}<div class="ph"></div>{/if}
            <div>
              <strong>{c.title}</strong>
              <div>{c.authors.join(', ')}</div>
              <div class="muted">
                {[c.publisher, c.pubdate, c.isbn && `ISBN ${c.isbn}`, label[c.source]].filter(Boolean).join(' · ')}
              </div>
            </div>
          </label>
        {:else}
          <p class="muted">Nada encontrado</p>
        {/each}
        {#each result.errors as e}<p class="err">{e}</p>{/each}
      {/if}
    </div>
    <div class="opts">
      <label><input type="checkbox" bind:checked={useMeta} /> Metadados</label>
      <label><input type="checkbox" bind:checked={useCover} disabled={!chosen?.coverUrl} /> Capa</label>
    </div>
    {#if error}<p class="err">{error}</p>{/if}
    <div class="btns">
      <button onclick={() => onclose(false)} disabled={busy}>Cancelar</button>
      <button onclick={apply} disabled={!chosen || busy || (!useMeta && !(useCover && chosen.coverUrl))}>
        {busy ? 'Aplicando…' : 'Aplicar'}
      </button>
    </div>
  </div>
</div>

<style>
  .backdrop { position: fixed; inset: 0; background: rgba(0, 0, 0, 0.45); display: grid; place-items: center; z-index: 10; }
  .dialog { background: var(--surface); color: var(--fg); width: min(720px, 92vw); max-height: 88vh; display: flex; flex-direction: column; gap: 10px; padding: 16px; border-radius: 10px; box-shadow: 0 10px 40px rgba(0,0,0,.4); }
  h2 { margin: 0; font-size: 1.05rem; }
  .list { overflow: auto; min-height: 120px; display: flex; flex-direction: column; gap: 6px; }
  .cand { display: flex; gap: 10px; align-items: center; padding: 6px; border: 1px solid var(--border); border-radius: 6px; cursor: pointer; }
  .cand.sel { border-color: var(--accent); }
  .cand img, .ph { width: 60px; min-height: 80px; object-fit: cover; background: var(--border); flex: none; }
  .muted { color: var(--muted); font-size: 0.85rem; }
  .err { color: #c0392b; margin: 2px 0; }
  .opts { display: flex; gap: 16px; }
  .btns { display: flex; justify-content: flex-end; gap: 8px; }
</style>
