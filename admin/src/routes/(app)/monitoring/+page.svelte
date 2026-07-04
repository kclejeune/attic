<script lang="ts">
	import { formatBytes, formatCount } from '$lib/format';
	import AreaChart from '$lib/components/charts/area-chart.svelte';
	import BarChart from '$lib/components/charts/bar-chart.svelte';

	let { data } = $props();
	const b = $derived(data.buckets);

	const storagePoints = $derived(
		b.map((w) => ({ label: w.weekStart, value: w.cumulativeBytes }))
	);
	const pushBars = $derived(b.map((w) => ({ label: w.weekStart, value: w.paths })));
	const totalPaths = $derived(b.reduce((n, w) => n + w.paths, 0));
	const peakWeek = $derived(b.reduce((m, w) => Math.max(m, w.paths), 0));
</script>

<div class="mx-auto max-w-6xl px-8 py-8">
	<header class="mb-8">
		<h1 class="text-2xl font-semibold tracking-tight">Monitoring</h1>
		<p class="mt-1 text-sm text-muted-foreground">
			Storage and push activity over time, by week uploaded.
		</p>
	</header>

	{#if b.length === 0}
		<div class="rounded-lg border border-dashed py-16 text-center">
			<p class="text-sm text-muted-foreground">No activity to chart yet.</p>
		</div>
	{:else}
		<section class="mb-8 rounded-lg border bg-card p-5">
			<div class="mb-4 flex items-baseline justify-between">
				<h2 class="text-sm font-medium">Storage growth</h2>
				<span class="font-mono text-sm text-muted-foreground">
					{formatBytes(b[b.length - 1].cumulativeBytes)} total
				</span>
			</div>
			<AreaChart points={storagePoints} format={formatBytes} />
		</section>

		<section class="rounded-lg border bg-card p-5">
			<div class="mb-4 flex items-baseline justify-between">
				<h2 class="text-sm font-medium">Store paths added</h2>
				<span class="text-sm text-muted-foreground">
					<span class="font-mono text-foreground">{formatCount(totalPaths)}</span> total ·
					peak <span class="font-mono text-foreground">{formatCount(peakWeek)}</span>/wk
				</span>
			</div>
			<BarChart bars={pushBars} format={formatCount} />
		</section>
	{/if}
</div>
