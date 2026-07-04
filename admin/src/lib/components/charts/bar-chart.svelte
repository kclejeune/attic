<script lang="ts">
	interface Bar {
		label: string;
		value: number;
	}

	let {
		bars,
		format = (v: number) => String(v)
	}: { bars: Bar[]; format?: (v: number) => string } = $props();

	const W = 720,
		H = 220,
		padL = 44,
		padR = 14,
		padT = 14,
		padB = 30;
	const innerW = W - padL - padR;
	const innerH = H - padT - padB;

	const max = $derived(Math.max(1, ...bars.map((b) => b.value)));
	const slot = $derived(bars.length ? innerW / bars.length : innerW);
	const barW = $derived(Math.max(2, Math.min(28, slot - 6)));
	const xAt = (i: number) => padL + i * slot + (slot - barW) / 2;
	const hAt = (v: number) => (v / max) * innerH;

	const ticks = $derived([0, 0.5, 1].map((f) => ({ v: f * max, y: padT + innerH - f * innerH })));
	const xLabels = $derived(
		bars.length
			? [0, Math.floor((bars.length - 1) / 2), bars.length - 1]
					.filter((i, idx, a) => a.indexOf(i) === idx)
					.map((i) => ({ x: xAt(i) + barW / 2, label: bars[i].label }))
			: []
	);

	let hovered = $state<number | null>(null);
</script>

<div class="relative">
	<svg viewBox="0 0 {W} {H}" class="w-full" role="img" aria-label="Store paths added per week">
		{#each ticks as t (t.v)}
			<line x1={padL} x2={W - padR} y1={t.y} y2={t.y} class="stroke-border" stroke-width="1" />
			<text x={padL - 8} y={t.y + 4} text-anchor="end" class="fill-muted-foreground text-[11px]">
				{format(t.v)}
			</text>
		{/each}

		{#each bars as bar, i (i)}
			<rect
				x={xAt(i)}
				y={padT + innerH - hAt(bar.value)}
				width={barW}
				height={Math.max(0, hAt(bar.value))}
				rx="3"
				class="transition-colors {hovered === i ? 'fill-primary' : 'fill-primary/70'}"
				onpointerenter={() => (hovered = i)}
				onpointerleave={() => (hovered = null)}
				role="presentation"
			/>
		{/each}

		{#each xLabels as l (l.x)}
			<text x={l.x} y={H - 10} text-anchor="middle" class="fill-muted-foreground text-[11px]">
				{l.label}
			</text>
		{/each}
	</svg>

	{#if hovered !== null && bars[hovered]}
		{@const frac = (xAt(hovered) + barW / 2) / W}
		<div
			class="pointer-events-none absolute top-0 -translate-x-1/2 rounded-md border bg-popover px-2.5 py-1.5 text-xs shadow-md"
			style="left: {frac * 100}%"
		>
			<div class="font-medium">{format(bars[hovered].value)}</div>
			<div class="text-muted-foreground">{bars[hovered].label}</div>
		</div>
	{/if}
</div>
