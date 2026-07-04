<script lang="ts">
	import { Button } from '$lib/components/ui/button/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { TerminalSquare } from '@lucide/svelte';

	let { data, form } = $props();
</script>

<div class="flex min-h-svh items-center justify-center bg-background px-4">
	<div class="w-full max-w-md">
		<div class="mb-6 flex flex-col items-center text-center">
			<div
				class="mb-4 flex size-12 items-center justify-center rounded-xl bg-accent text-accent-foreground"
			>
				<TerminalSquare class="size-6" />
			</div>
			<h1 class="text-xl font-semibold tracking-tight">Authorize the attic CLI</h1>
			<p class="mt-1 text-sm text-muted-foreground">
				Signed in as {data.user.email ?? data.user.name}. A token for
				<span class="font-mono">{data.hostname || 'this machine'}</span> will be created and sent to
				the CLI waiting on <span class="font-mono">127.0.0.1:{data.port}</span>.
			</p>
		</div>

		<form
			method="POST"
			action="?/authorize"
			class="space-y-5 rounded-lg border bg-card p-6"
		>
			<input type="hidden" name="port" value={data.port} />
			<input type="hidden" name="state" value={data.state} />

			<div class="space-y-2">
				<Label for="label">Token name</Label>
				<Input id="label" name="label" value={data.label} />
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
					<input name="push" type="checkbox" checked class="size-4 rounded border-input text-primary" />
					Push
				</label>
			</div>

			{#if form?.error}
				<p class="text-sm text-destructive">{form.error}</p>
			{/if}

			<div class="flex gap-3 pt-1">
				<Button type="submit" class="flex-1">Authorize</Button>
			</div>
			<p class="text-center text-xs text-muted-foreground">
				You can revoke this token any time from the Tokens screen.
			</p>
		</form>
	</div>
</div>
