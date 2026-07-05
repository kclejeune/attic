<script lang="ts">
	import { formatBytes, formatCount } from '$lib/format';
	import { goto } from '$app/navigation';
	import AreaChart from '$lib/components/charts/area-chart.svelte';
	import BarChart from '$lib/components/charts/bar-chart.svelte';

	let { data } = $props();
	const b = $derived(data.buckets);

	const storagePoints = $derived(b.map((w) => ({ label: w.date, value: w.cumulativeBytes })));
	const pushBars = $derived(b.map((w) => ({ label: w.date, value: w.paths })));
	const totalPaths = $derived(b.reduce((n, w) => n + w.paths, 0));
	const peak = $derived(b.reduce((m, w) => Math.max(m, w.paths), 0));

	const unit = $derived(
		data.granularity === 'day' ? 'day' : data.granularity === 'month' ? 'mo' : 'wk'
	);

	const RANGES: [string, string][] = [
		['30d', '30d'],
		['90d', '90d'],
		['6m', '6m'],
		['1y', '1y'],
		['all', 'All']
	];
	const GRANULARITIES: [string, string][] = [
		['day', 'Day'],
		['week', 'Week'],
		['month', 'Month']
	];

	function setParam(next: { range?: string; granularity?: string }) {
		const range = next.range ?? data.range;
		const granularity = next.granularity ?? data.granularity;
		const params = new URLSearchParams();
		if (range !== 'all') params.set('range', range);
		if (granularity !== 'week') params.set('granularity', granularity);
		const qs = params.toString();
		goto(qs ? `?${qs}` : '?', { replaceState: true, noScroll: true, keepFocus: true });
	}
</script>

{#snippet segmented(
	options: [string, string][],
	active: string,
	pick: (v: string) => void
)}
	<div class="inline-flex rounded-md border border-input p-0.5 text-xs">
		{#each options as [val, label] (val)}
			<button
				type="button"
				onclick={() => pick(val)}
				class="rounded px-2.5 py-1 transition-colors {active === val
					? 'bg-muted font-medium text-foreground'
					: 'text-muted-foreground hover:text-foreground'}"
			>
				{label}
			</button>
		{/each}
	</div>
{/snippet}

<div class="mx-auto max-w-6xl px-8 py-8">
	<header class="mb-8 flex flex-wrap items-end justify-between gap-4">
		<div>
			<h1 class="text-2xl font-semibold tracking-tight">Monitoring</h1>
			<p class="mt-1 text-sm text-muted-foreground">Storage and push activity over time.</p>
		</div>
		<div class="flex flex-wrap items-center gap-2">
			{@render segmented(RANGES, data.range, (v) => setParam({ range: v }))}
			{@render segmented(GRANULARITIES, data.granularity, (v) => setParam({ granularity: v }))}
		</div>
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
					peak <span class="font-mono text-foreground">{formatCount(peak)}</span>/{unit}
				</span>
			</div>
			<BarChart bars={pushBars} format={formatCount} />
		</section>
	{/if}
</div>
