<script lang="ts">
	import { enhance } from '$app/forms';
	import { formatBytes, formatCount } from '$lib/format';
	import * as Card from '$lib/components/ui/card/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Trash2, Check } from '@lucide/svelte';

	let { data, form } = $props();
	const s = $derived(data.stats);
	let running = $state(false);

	const tiles = $derived([
		{ label: 'Caches', value: formatCount(s.caches), mono: false },
		{ label: 'Store paths', value: formatCount(s.objects), mono: false },
		{ label: 'NARs stored', value: formatCount(s.nars), mono: false },
		{ label: 'Storage used', value: formatBytes(s.storageBytes), mono: true }
	]);

	const reclaimable = $derived(s.pendingNars + s.orphanNars + s.orphanChunks);
</script>

<div class="mx-auto max-w-6xl px-8 py-8">
	<header class="mb-8">
		<h1 class="text-2xl font-semibold tracking-tight">Overview</h1>
		<p class="mt-1 text-sm text-muted-foreground">
			Content-addressed storage across all caches in this deployment.
		</p>
	</header>

	<div class="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-4">
		{#each tiles as tile (tile.label)}
			<Card.Root>
				<Card.Header class="pb-2">
					<Card.Description>{tile.label}</Card.Description>
				</Card.Header>
				<Card.Content>
					<div class="text-3xl font-semibold tracking-tight {tile.mono ? 'font-mono' : ''}">
						{tile.value}
					</div>
				</Card.Content>
			</Card.Root>
		{/each}
	</div>

	<section class="mt-8 rounded-lg border bg-card p-5">
		<div class="flex items-start justify-between gap-4">
			<div>
				<h2 class="text-sm font-medium">Garbage collection</h2>
				<p class="mt-1 text-sm text-muted-foreground">
					Runs nightly. Reaps abandoned uploads, retention-expired paths, and unreferenced
					NARs and chunks.
				</p>
			</div>
			<form
				method="POST"
				action="?/gc"
				use:enhance={() => {
					running = true;
					return async ({ update }) => {
						await update();
						running = false;
					};
				}}
			>
				<Button type="submit" variant="outline" disabled={running}>
					<Trash2 class="size-4" />
					{running ? 'Running…' : 'Run now'}
				</Button>
			</form>
		</div>

		<dl class="mt-4 grid grid-cols-3 gap-4 border-t pt-4 text-sm">
			<div>
				<dt class="text-xs text-muted-foreground">Pending uploads</dt>
				<dd class="mt-0.5 font-mono">{formatCount(s.pendingNars)}</dd>
			</div>
			<div>
				<dt class="text-xs text-muted-foreground">Orphan NARs</dt>
				<dd class="mt-0.5 font-mono">{formatCount(s.orphanNars)}</dd>
			</div>
			<div>
				<dt class="text-xs text-muted-foreground">Orphan chunks</dt>
				<dd class="mt-0.5 font-mono">{formatCount(s.orphanChunks)}</dd>
			</div>
		</dl>

		{#if form?.gcError}
			<p class="mt-4 text-sm text-destructive">{form.gcError}</p>
		{:else if form?.gcStats}
			<p class="mt-4 inline-flex items-center gap-1.5 text-sm text-muted-foreground">
				<Check class="size-4 text-primary" />
				Reclaimed {formatCount(
					(form.gcStats.abandoned_uploads_reaped ?? 0) +
						(form.gcStats.expired_objects_reaped ?? 0) +
						(form.gcStats.orphan_nars_reaped ?? 0) +
						(form.gcStats.orphan_chunks_reaped ?? 0)
				)} items ({formatCount(form.gcStats.orphan_chunks_reaped ?? 0)} chunks freed from storage).
			</p>
		{:else if reclaimable === 0}
			<p class="mt-4 text-sm text-muted-foreground">Nothing to reclaim right now.</p>
		{/if}
	</section>
</div>
