<script lang="ts">
	import { formatBytes, formatCount } from '$lib/format';
	import { Button } from '$lib/components/ui/button/index.js';
	import CopyButton from '$lib/components/copy-button.svelte';
	import CopyField from '$lib/components/copy-field.svelte';
	import { ArrowLeft, Lock, Globe, ChevronLeft, ChevronRight } from '@lucide/svelte';

	let { data } = $props();
	const c = $derived(data.cache);

	const nixConf = $derived(
		c.publicKey
			? `extra-substituters = ${c.url}\nextra-trusted-public-keys = ${c.publicKey}`
			: `extra-substituters = ${c.url}`
	);

	function shortHash(path: string): string {
		// Trim the /nix/store/<hash>- prefix for readability; keep the human name.
		return path.replace(/^\/nix\/store\//, '');
	}
</script>

<div class="mx-auto max-w-6xl px-8 py-8">
	<a
		href="/caches"
		class="mb-6 inline-flex items-center gap-1.5 text-sm text-muted-foreground hover:text-foreground"
	>
		<ArrowLeft class="size-4" /> Caches
	</a>

	<header class="mb-8 flex flex-wrap items-start justify-between gap-4">
		<div>
			<h1 class="font-mono text-2xl font-semibold tracking-tight">{c.name}</h1>
			<div class="mt-2 flex items-center gap-4 text-sm text-muted-foreground">
				{#if c.isPublic}
					<span class="inline-flex items-center gap-1.5"><Globe class="size-3.5" /> Public</span>
				{:else}
					<span class="inline-flex items-center gap-1.5"><Lock class="size-3.5" /> Private</span>
				{/if}
				<span>Priority <span class="font-mono text-foreground">{c.priority}</span></span>
				<span>Compression <span class="font-mono text-foreground">{c.compression}</span></span>
				<span>
					Retention
					<span class="font-mono text-foreground"
						>{c.retentionDays ? `${c.retentionDays}d` : 'none'}</span
					>
				</span>
			</div>
		</div>
		<Button variant="outline" href="/caches/{c.name}/settings">Configure</Button>
	</header>

	<section class="mb-8 rounded-lg border bg-card p-5">
		<h2 class="text-sm font-medium">Trust this cache</h2>
		<p class="mt-1 text-sm text-muted-foreground">
			Add to <span class="font-mono">nix.conf</span>, or a flake's
			<span class="font-mono">nixConfig</span>, to pull from this cache.
		</p>

		<dl class="mt-4 space-y-3">
			<div>
				<dt class="mb-1 text-xs text-muted-foreground">Substituter URL</dt>
				<dd><CopyField text={c.url} label="Copy URL" /></dd>
			</div>
			<div>
				<dt class="mb-1 text-xs text-muted-foreground">Trusted public key</dt>
				<dd>
					{#if c.publicKey}
						<CopyField text={c.publicKey} label="Copy public key" />
					{:else}
						<div class="rounded-md border border-input bg-muted px-3 py-2.5 font-mono text-xs text-muted-foreground">
							unavailable
						</div>
					{/if}
				</dd>
			</div>
		</dl>

		<div class="mt-4">
			<span class="mb-1 block text-xs text-muted-foreground">nix.conf</span>
			<div class="relative overflow-hidden rounded-md border border-input bg-muted">
				<pre class="overflow-x-auto px-3 py-2.5 pr-12 font-mono text-xs leading-5"><code
						>{nixConf}</code
					></pre>
				<div class="absolute top-1.5 right-1.5">
					<CopyButton text={nixConf} label="Copy nix.conf snippet" />
				</div>
			</div>
		</div>
	</section>

	<div class="mb-3 flex items-baseline justify-between">
		<h2 class="text-sm font-medium">Store paths</h2>
		<span class="text-sm text-muted-foreground">{formatCount(data.total)} total</span>
	</div>

	{#if data.paths.length === 0}
		<div class="rounded-lg border border-dashed py-16 text-center">
			<p class="text-sm text-muted-foreground">No store paths in this cache yet.</p>
		</div>
	{:else}
		<div class="overflow-hidden rounded-lg border">
			<table class="w-full text-sm">
				<thead class="border-b bg-muted/40 text-left text-xs text-muted-foreground">
					<tr>
						<th class="px-4 py-2.5 font-medium">Store path</th>
						<th class="px-4 py-2.5 text-right font-medium">NAR size</th>
					</tr>
				</thead>
				<tbody class="divide-y">
					{#each data.paths as p (p.hash)}
						<tr class="transition-colors hover:bg-muted/30">
							<td class="px-4 py-2.5 font-mono text-xs">{shortHash(p.storePath)}</td>
							<td class="px-4 py-2.5 text-right font-mono">{formatBytes(p.narSize)}</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>

		<div class="mt-4 flex items-center justify-between text-sm">
			<span class="text-muted-foreground">
				{data.page * data.pageSize + 1}–{data.page * data.pageSize + data.paths.length}
				of {formatCount(data.total)}
			</span>
			<div class="flex gap-2">
				<Button
					variant="outline"
					size="sm"
					disabled={data.page === 0}
					href={data.page > 0 ? `?page=${data.page - 1}` : undefined}
				>
					<ChevronLeft class="size-4" /> Prev
				</Button>
				<Button
					variant="outline"
					size="sm"
					disabled={!data.hasMore}
					href={data.hasMore ? `?page=${data.page + 1}` : undefined}
				>
					Next <ChevronRight class="size-4" />
				</Button>
			</div>
		</div>
	{/if}
</div>
