<script lang="ts">
	import { formatBytes, formatCount } from '$lib/format';
	import * as Card from '$lib/components/ui/card/index.js';

	let { data } = $props();
	const s = $derived(data.stats);

	const tiles = $derived([
		{ label: 'Caches', value: formatCount(s.caches), mono: false },
		{ label: 'Store paths', value: formatCount(s.objects), mono: false },
		{ label: 'NARs stored', value: formatCount(s.nars), mono: false },
		{ label: 'Storage used', value: formatBytes(s.storageBytes), mono: true }
	]);
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

	{#if s.pendingNars > 0}
		<p class="mt-6 text-sm text-muted-foreground">
			<span class="font-mono font-medium text-foreground">{formatCount(s.pendingNars)}</span>
			NAR{s.pendingNars === 1 ? '' : 's'} pending upload — these are reaped by garbage collection if
			abandoned.
		</p>
	{/if}
</div>
