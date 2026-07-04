<script lang="ts">
	import { Button } from '$lib/components/ui/button/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { TerminalSquare, Check } from '@lucide/svelte';

	let { data, form } = $props();
	const canApprove = $derived(data.grant && data.grant.status === 'pending' && !data.grant.expired);
</script>

<div class="flex min-h-svh items-center justify-center bg-background px-4">
	<div class="w-full max-w-md">
		<div class="mb-6 flex flex-col items-center text-center">
			<div
				class="mb-4 flex size-12 items-center justify-center rounded-xl bg-accent text-accent-foreground"
			>
				<TerminalSquare class="size-6" />
			</div>
			<h1 class="text-xl font-semibold tracking-tight">Device login</h1>
			<p class="mt-1 text-sm text-muted-foreground">
				Signed in as {data.user.email ?? data.user.name}. Enter the code shown in your terminal.
			</p>
		</div>

		{#if form?.approved}
			<div class="rounded-lg border border-primary/40 bg-accent/40 p-6 text-center">
				<Check class="mx-auto mb-2 size-6 text-primary" />
				<p class="text-sm font-medium">Approved</p>
				<p class="mt-1 text-sm text-muted-foreground">
					Return to your terminal — the CLI will finish signing in.
				</p>
			</div>
		{:else if !data.code}
			<form method="GET" class="space-y-4 rounded-lg border bg-card p-6">
				<div class="space-y-2">
					<Label for="code">Device code</Label>
					<Input id="code" name="code" placeholder="XXXX-XXXX" autocomplete="off" autofocus />
				</div>
				<Button type="submit" class="w-full">Continue</Button>
			</form>
		{:else if data.notFound}
			<div class="rounded-lg border bg-card p-6 text-center text-sm text-muted-foreground">
				No pending login found for <span class="font-mono text-foreground">{data.code}</span>.
				<a href="/cli/device" class="text-primary hover:underline">Try again</a>.
			</div>
		{:else if data.grant && (data.grant.status !== 'pending' || data.grant.expired)}
			<div class="rounded-lg border bg-card p-6 text-center text-sm text-muted-foreground">
				{data.grant.expired ? 'This code has expired.' : 'This code was already used.'}
				<a href="/cli/device" class="text-primary hover:underline">Start again</a>.
			</div>
		{:else}
			<form method="POST" action="?/approve" class="space-y-5 rounded-lg border bg-card p-6">
				<input type="hidden" name="user_code" value={data.code} />
				<p class="text-sm text-muted-foreground">
					Authorizing device code
					<span class="font-mono font-medium text-foreground">{data.code}</span>.
				</p>

				<div class="space-y-2">
					<Label for="label">Token name</Label>
					<Input id="label" name="label" value="attic CLI" />
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
						<Label for="expiry_days">Expires (days)</Label>
						<Input id="expiry_days" name="expiry_days" type="number" value="90" min="1" max="3650" />
					</div>
				</div>

				<div class="flex gap-6">
					<label class="flex items-center gap-2 text-sm">
						<input name="pull" type="checkbox" checked class="size-4 rounded border-input text-primary" />
						Pull
					</label>
					<label class="flex items-center gap-2 text-sm">
						<input name="push" type="checkbox" checked class="size-4 rounded border-input text-primary" />
						Push
					</label>
				</div>

				{#if form?.error}
					<p class="text-sm text-destructive">{form.error}</p>
				{/if}

				<Button type="submit" class="w-full" disabled={!canApprove}>Authorize</Button>
			</form>
		{/if}
	</div>
</div>
