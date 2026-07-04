<script lang="ts">
	import { enhance } from '$app/forms';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import CopyButton from '$lib/components/copy-button.svelte';
	import { KeyRound, Trash2, TriangleAlert } from '@lucide/svelte';

	let { data, form } = $props();
	let issuing = $state(false);

	function scopeLabel(scopeJson: string): string {
		try {
			const s = JSON.parse(scopeJson) as Record<string, { r?: number; w?: number }>;
			return Object.entries(s)
				.map(([cache, p]) => {
					const perms = [p.r && 'pull', p.w && 'push'].filter(Boolean).join('+');
					return `${cache === '*' ? 'all caches' : cache} · ${perms}`;
				})
				.join(', ');
		} catch {
			return scopeJson;
		}
	}

	function fmtDate(unix: number | null): string {
		if (!unix) return 'never';
		return new Date(unix * 1000).toISOString().slice(0, 10);
	}
</script>

<div class="mx-auto max-w-6xl px-8 py-8">
	<header class="mb-8">
		<h1 class="text-2xl font-semibold tracking-tight">Tokens</h1>
		<p class="mt-1 text-sm text-muted-foreground">
			Scoped API tokens for CI and programmatic access. Revoking one takes effect immediately.
		</p>
	</header>

	{#if form?.issued}
		<div class="mb-8 rounded-lg border border-primary/40 bg-accent/40 p-4">
			<div class="mb-2 flex items-center gap-2 text-sm font-medium">
				<TriangleAlert class="size-4 text-primary" />
				Copy “{form.issued.name}” now — it won't be shown again.
			</div>
			<div class="flex items-center gap-2">
				<code class="min-w-0 flex-1 truncate rounded-md bg-background px-3 py-2 font-mono text-xs">
					{form.issued.token}
				</code>
				<CopyButton text={form.issued.token} label="Copy token" />
			</div>
		</div>
	{/if}

	<section class="mb-10 rounded-lg border bg-card p-5">
		<h2 class="mb-4 text-sm font-medium">Issue a token</h2>
		<form
			method="POST"
			action="?/issue"
			use:enhance={() => {
				issuing = true;
				return async ({ update }) => {
					await update();
					issuing = false;
				};
			}}
			class="space-y-4"
		>
			<div class="space-y-2">
				<Label for="name">Name</Label>
				<Input id="name" name="name" placeholder="ci-deploy" />
			</div>

			<div class="grid grid-cols-2 gap-4">
				<div class="space-y-2">
					<Label for="cache">Cache</Label>
					<select
						id="cache"
						name="cache"
						class="flex h-9 w-full rounded-md border border-input bg-transparent px-3 py-1 text-sm focus-visible:ring-1 focus-visible:ring-ring focus-visible:outline-none"
					>
						<option value="*">All caches</option>
						{#each data.cacheNames as name (name)}
							<option value={name}>{name}</option>
						{/each}
					</select>
				</div>
				<div class="space-y-2">
					<Label for="expiry_days">Expires in (days)</Label>
					<Input id="expiry_days" name="expiry_days" type="number" value="90" min="1" max="3650" />
				</div>
			</div>

			<div class="flex gap-6">
				<label class="flex items-center gap-2 text-sm">
					<input name="pull" type="checkbox" checked class="size-4 rounded border-input text-primary" />
					Pull
				</label>
				<label class="flex items-center gap-2 text-sm">
					<input name="push" type="checkbox" class="size-4 rounded border-input text-primary" />
					Push
				</label>
			</div>

			{#if form?.error}
				<p class="text-sm text-destructive">{form.error}</p>
			{/if}

			<Button type="submit" disabled={issuing}>{issuing ? 'Issuing…' : 'Issue token'}</Button>
		</form>
	</section>

	<h2 class="mb-3 text-sm font-medium">Your tokens</h2>
	{#if data.tokens.length === 0}
		<div class="rounded-lg border border-dashed py-12 text-center">
			<KeyRound class="mx-auto mb-3 size-6 text-muted-foreground" />
			<p class="text-sm text-muted-foreground">No tokens yet.</p>
		</div>
	{:else}
		<div class="divide-y rounded-lg border">
			{#each data.tokens as t (t.id)}
				<div class="flex items-center gap-4 px-4 py-3">
					<div class="min-w-0 flex-1">
						<div class="flex items-center gap-2">
							<span class="font-medium">{t.name}</span>
							{#if t.status === 'revoked'}
								<span class="rounded bg-destructive/10 px-1.5 py-0.5 text-xs text-destructive"
									>revoked</span
								>
							{:else if t.status === 'expired'}
								<span class="rounded bg-muted px-1.5 py-0.5 text-xs text-muted-foreground"
									>expired</span
								>
							{/if}
						</div>
						<div class="mt-0.5 text-xs text-muted-foreground">
							{scopeLabel(t.scope)} · expires {fmtDate(t.expiresAt)}
						</div>
					</div>
					{#if t.status === 'active'}
						<form method="POST" action="?/revoke" use:enhance>
							<input type="hidden" name="id" value={t.id} />
							<Button
								type="submit"
								variant="ghost"
								size="sm"
								class="text-muted-foreground hover:text-destructive"
							>
								<Trash2 class="size-4" /> Revoke
							</Button>
						</form>
					{/if}
				</div>
			{/each}
		</div>
	{/if}
</div>
