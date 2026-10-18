<script lang="ts">
  import { onMount } from 'svelte';
  import { convertFileSrc } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';
  import { getCurrentWebview } from '@tauri-apps/api/webview';
  import { open } from '@tauri-apps/plugin-dialog';
  import {
    ADD_EXTENSIONS, addBooks, autoMetadata, deviceStatus, libraryPath, listBooks, sendBooks,
    type BookRow, type DeviceStatus, type SendProgress
  } from '$lib/api';
  import MetadataDialog from '$lib/MetadataDialog.svelte';

  let books = $state<BookRow[]>([]);
  let path = $state('');
  let query = $state('');
  let checked = $state<Set<number>>(new Set());
  let status = $state<DeviceStatus>({ connected: false, deviceName: null, port: 0, discovery: true, onDevice: [] });
  let message = $state('');
  let sendLine = $state('');
  let errors = $state<string[]>([]);
  let dragging = $state(false);
  let metaBook = $state<BookRow | null>(null);

  const norm = (s: string) => s.normalize('NFD').replace(/\p{Diacritic}/gu, '').toLowerCase();
  const visible = $derived.by(() => {
    const q = norm(query.trim());
    if (!q) return books;
    return books.filter((b) => norm([b.title, ...b.authors, b.series ?? '', ...b.tags].join(' ')).includes(q));
  });
  const onDevice = $derived(new Set(status.onDevice));

  async function reload() {
    books = await listBooks();
  }

  async function add(paths: string[]) {
    if (!paths.length) return;
    try {
      const r = await addBooks(paths);
      message = `${r.added} adicionado(s)`;
      errors = r.skipped.map((s) => `${s.file}: ${s.reason}`);
      await reload();
      await auto(r.ids);
    } catch (e) {
      errors = [String(e)];
    }
  }

  async function auto(ids: number[]) {
    if (!ids.length) return;
    sendLine = `Baixando metadados de ${ids.length} livro(s)…`;
    try {
      const r = await autoMetadata(ids);
      message = `${message} · ${r.updated} com metadados`;
      errors = [...errors, ...r.failed.map((f) => `Metadados: ${f}`)];
      await reload();
    } catch (e) {
      errors = [...errors, String(e)];
    }
    sendLine = '';
  }

  async function pick() {
    const sel = await open({ multiple: true, filters: [{ name: 'Livros', extensions: ADD_EXTENSIONS }] });
    if (sel) await add(sel);
  }

  async function send() {
    errors = [];
    try {
      await sendBooks([...checked]);
    } catch (e) {
      errors = [String(e)];
    }
  }

  function toggle(id: number) {
    const next = new Set(checked);
    if (!next.delete(id)) next.add(id);
    checked = next;
  }

  const allChecked = $derived(visible.length > 0 && visible.every((b) => checked.has(b.id)));
  function toggleAll() {
    checked = allChecked ? new Set() : new Set(visible.map((b) => b.id));
  }

  onMount(() => {
    libraryPath().then((p) => (path = p));
    deviceStatus().then((s) => (status = s));
    reload();
    const unlisten = [
      listen<DeviceStatus>('device-status', (e) => (status = e.payload)),
      listen<SendProgress>('send-progress', (e) => {
        const p = e.payload;
        if (p.error) errors = [...errors, `${p.title}: ${p.error}`];
        sendLine = p.done ? '' : `Enviando ${p.current}/${p.total}: ${p.title}`;
      }),
      getCurrentWebview().onDragDropEvent((e) => {
        const t = e.payload.type;
        dragging = t === 'over' || t === 'enter';
        if (t === 'drop') add(e.payload.paths);
      })
    ];
    return () => unlisten.forEach((u) => u.then((f) => f()));
  });
</script>

<main>
  <header>
    <img class="logo" src="/logo.png" alt="" width="36" height="36" />
    <div class="title">
      <strong>Calibre Lite</strong>
      <small class="muted" title={path}>{path}</small>
    </div>
    <div class="search">
      <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2"><circle cx="11" cy="11" r="7" /><path d="M20 20l-4-4" /></svg>
      <input type="search" placeholder="Buscar por título, autor, série ou etiqueta" bind:value={query} />
    </div>
    <button class="primary" onclick={pick}>+ Adicionar livros</button>
  </header>

  <div class="bar">
    <label class="all">
      <input type="checkbox" checked={allChecked} onchange={toggleAll} disabled={visible.length === 0} />
      {checked.size > 0 ? `${checked.size} selecionado(s)` : `${visible.length} livro(s)`}
    </label>
    <button onclick={send} disabled={!status.connected || checked.size === 0}
      title={status.connected ? '' : 'Conecte o KOReader primeiro'}>Enviar para KOReader</button>
    <button onclick={() => auto([...checked])} disabled={checked.size === 0 || !!sendLine}>Metadados automáticos</button>
    <button onclick={() => (metaBook = books.find((b) => b.id === [...checked][0]) ?? null)} disabled={checked.size !== 1}
      title={checked.size === 1 ? '' : 'Selecione exatamente um livro'}>Baixar metadados</button>
    <span class="status">{sendLine || message}</span>
    <span class="pill" class:on={status.connected} title={status.discovery ? '' : 'Descoberta automática indisponível'}>
      <i></i>
      {#if status.connected}
        KOReader: {status.deviceName}
      {:else}
        Aguardando KOReader (porta {status.port})
      {/if}
    </span>
  </div>

  {#if errors.length}
    <div class="errs">
      <button class="x" aria-label="Fechar" onclick={() => (errors = [])}>×</button>
      {#each errors as e}<div>{e}</div>{/each}
    </div>
  {/if}

  <ul>
    {#each visible as b (b.id)}
      <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
      <li class:sel={checked.has(b.id)} onclick={() => toggle(b.id)}>
        <input type="checkbox" checked={checked.has(b.id)} onclick={(e) => e.stopPropagation()} onchange={() => toggle(b.id)} />
        {#if b.coverPath}
          <img class="cover" src={convertFileSrc(b.coverPath) + '?t=' + encodeURIComponent(b.lastModified)} alt="" width="48" height="68" loading="lazy" />
        {:else}
          <div class="cover ph">{b.formats[0] ?? ''}</div>
        {/if}
        <div class="info">
          <div class="t">{b.title}</div>
          <div class="a">{b.authors.join(', ')}</div>
          {#if b.series}<div class="muted">{b.series} [{b.seriesIndex}]</div>{/if}
        </div>
        <div class="chips">
          {#each b.formats as f}<span class="chip">{f}</span>{/each}
          {#if onDevice.has(b.uuid)}<span class="chip dev">No KOReader</span>{/if}
        </div>
      </li>
    {:else}
      <li class="empty">
        {#if books.length}
          Nenhum resultado para “{query}”.
        {:else}
          <img src="/logo.png" alt="" width="64" height="64" />
          <p>Sua biblioteca está vazia.</p>
          <p class="muted">Arraste arquivos para cá ou use “Adicionar livros”.</p>
        {/if}
      </li>
    {/each}
  </ul>

  {#if dragging}<div class="drop"><div>Solte para adicionar à biblioteca</div></div>{/if}
  {#if metaBook}
    <MetadataDialog
      book={metaBook}
      onclose={(changed) => {
        metaBook = null;
        if (changed) reload();
      }}
    />
  {/if}
</main>

<style>
  :global(:root) {
    --bg: #fafafa; --surface: #fff; --fg: #1d1d1f; --muted: #777; --border: #e2e2e6; --accent: #2f6fed; --hover: #f1f4fb; --selbg: #e8f0fe;
    font-family: 'Segoe UI', system-ui, sans-serif; color: var(--fg); background: var(--bg);
    color-scheme: light dark;
  }
  @media (prefers-color-scheme: dark) {
    :global(:root) { --bg: #1b1b1d; --surface: #232326; --fg: #eee; --muted: #9a9aa0; --border: #34343a; --accent: #5b8dff; --hover: #2a2a2f; --selbg: #26335a; }
  }
  :global(body) { margin: 0; }
  :global(button) {
    font: inherit; font-size: 0.88rem; padding: 6px 12px; border-radius: 6px; cursor: pointer;
    background: var(--surface); color: var(--fg); border: 1px solid var(--border);
  }
  :global(button:hover:not(:disabled)) { border-color: var(--accent); }
  :global(button:disabled) { opacity: 0.45; cursor: default; }
  :global(button.primary) { background: var(--accent); border-color: var(--accent); color: #fff; }
  :global(input[type='checkbox']) { width: 16px; height: 16px; accent-color: var(--accent); cursor: pointer; }
  main { display: flex; flex-direction: column; height: 100vh; position: relative; }
  header { display: flex; gap: 12px; align-items: center; padding: 10px 16px; background: var(--surface); border-bottom: 1px solid var(--border); }
  .logo { flex: none; }
  .title { display: flex; flex-direction: column; min-width: 0; margin-right: auto; }
  .title small { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; max-width: 320px; }
  small, .muted { color: var(--muted); font-size: 0.8rem; }
  .search { position: relative; width: min(380px, 40vw); color: var(--muted); }
  .search svg { position: absolute; left: 10px; top: 50%; transform: translateY(-50%); }
  .search input { width: 100%; box-sizing: border-box; padding: 7px 10px 7px 32px; border-radius: 6px; border: 1px solid var(--border); background: var(--bg); color: var(--fg); font: inherit; font-size: 0.9rem; }
  .search input:focus { outline: 2px solid var(--accent); outline-offset: -1px; }
  .bar { display: flex; gap: 8px; align-items: center; padding: 8px 16px; border-bottom: 1px solid var(--border); }
  .all { display: flex; gap: 8px; align-items: center; font-size: 0.88rem; min-width: 130px; }
  .status { color: var(--muted); font-size: 0.85rem; margin-right: auto; margin-left: 6px; }
  .pill { display: flex; align-items: center; gap: 6px; padding: 3px 10px; border-radius: 99px; background: var(--border); font-size: 0.82rem; white-space: nowrap; }
  .pill i { width: 8px; height: 8px; border-radius: 50%; background: #c0392b; }
  .pill.on i { background: #2e9e5b; }
  .errs { position: relative; background: rgba(192, 57, 43, 0.12); color: #d9534f; padding: 8px 40px 8px 16px; font-size: 0.88rem; }
  .errs .x { position: absolute; right: 8px; top: 4px; border: 0; background: none; font-size: 1.2rem; padding: 0 6px; color: inherit; }
  ul { list-style: none; margin: 0; padding: 0; overflow: auto; flex: 1; }
  li { display: flex; gap: 14px; align-items: center; padding: 8px 16px; border-bottom: 1px solid var(--border); cursor: pointer; }
  li:hover { background: var(--hover); }
  li.sel { background: var(--selbg); }
  .cover { width: 48px; height: 68px; object-fit: cover; flex: none; border-radius: 3px; box-shadow: 0 1px 4px rgba(0, 0, 0, 0.3); }
  .ph { background: var(--border); display: grid; place-items: center; font-size: 0.7rem; color: var(--muted); box-shadow: none; }
  .info { flex: 1; min-width: 0; }
  .t { font-weight: 600; }
  .a { font-size: 0.9rem; }
  .chips { display: flex; gap: 4px; }
  .chip { font-size: 0.7rem; padding: 1px 7px; border-radius: 4px; border: 1px solid var(--border); }
  .chip.dev { background: #2e9e5b; color: #fff; border-color: #2e9e5b; }
  li.empty { flex-direction: column; justify-content: center; gap: 2px; cursor: default; padding: 60px 16px; border: 0; background: none; text-align: center; opacity: 0.9; }
  li.empty p { margin: 4px 0; }
  .drop { position: fixed; inset: 12px; display: grid; place-items: center; background: rgba(47, 111, 237, 0.18); border: 3px dashed var(--accent); border-radius: 12px; font-size: 1.3rem; pointer-events: none; z-index: 20; }
</style>
