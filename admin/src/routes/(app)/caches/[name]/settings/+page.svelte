<script lang="ts">
	import { enhance } from '$app/forms';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import { ArrowLeft, Check, Trash2 } from '@lucide/svelte';

	let { data, form } = $props();
	const c = $derived(data.cache);
	let submitting = $state(false);
	let renaming = $state(false);
	let deleting = $state(false);
</script>

<div class="mx-auto max-w-xl px-8 py-8">
	<a
		href="/caches/{c.name}"
		class="mb-6 inline-flex items-center gap-1.5 text-sm text-muted-foreground hover:text-foreground"
	>
		<ArrowLeft class="size-4" /> {c.name}
	</a>

	<header class="mb-8">
		<h1 class="text-2xl font-semibold tracking-tight">Settings</h1>
		<p class="mt-1 text-sm text-muted-foreground">
			Changing compression affects only newly pushed paths.
		</p>
	</header>

	<form
		method="POST"
		action="?/save"
		use:enhance={() => {
			submitting = true;
			return async ({ update }) => {
				await update({ reset: false });
				submitting = false;
			};
		}}
		class="space-y-6"
	>
		<div class="flex items-center gap-3">
			<input
				id="is_public"
				name="is_public"
				type="checkbox"
				checked={c.isPublic}
				class="size-4 rounded border-input text-primary focus:ring-ring"
			/>
			<Label for="is_public" class="font-normal">Public — anyone can pull without a token</Label>
		</div>

		<div class="grid grid-cols-2 gap-4">
			<div class="space-y-2">
				<Label for="priority">Priority</Label>
				<Input id="priority" name="priority" type="number" value={c.priority} />
			</div>
			<div class="space-y-2">
				<Label for="compression">Compression</Label>
				<select
					id="compression"
					name="compression"
					value={c.compression}
					class="flex h-9 w-full rounded-md border border-input bg-transparent px-3 py-1 text-sm shadow-xs focus-visible:ring-1 focus-visible:ring-ring focus-visible:outline-none"
				>
					<option value="zstd">zstd</option>
					<option value="br">brotli</option>
					<option value="xz">xz</option>
					<option value="none">none</option>
				</select>
			</div>
		</div>

		<div class="space-y-2">
			<Label for="retention_period">Retention (days)</Label>
			<Input
				id="retention_period"
				name="retention_period"
				type="number"
				placeholder="Leave blank for no automatic expiry"
				value={c.retentionDays ?? ''}
			/>
		</div>

		{#if form?.error}
			<p class="text-sm text-destructive">{form.error}</p>
		{/if}

		<div class="flex items-center gap-3">
			<Button type="submit" disabled={submitting}>
				{submitting ? 'Saving…' : 'Save changes'}
			</Button>
			{#if form?.saved}
				<span class="inline-flex items-center gap-1.5 text-sm text-muted-foreground">
					<Check class="size-4" /> Saved
				</span>
			{/if}
		</div>
	</form>

	<hr class="my-10 border-border" />

	<section class="space-y-4">
		<div>
			<h2 class="text-sm font-medium">Rename cache</h2>
			<p class="mt-1 text-sm text-muted-foreground">
				The signing key is preserved, so already-pushed paths stay trusted. The pull URL changes to
				the new name.
			</p>
		</div>
		<form
			method="POST"
			action="?/rename"
			use:enhance={() => {
				renaming = true;
				return async ({ update }) => {
					await update({ reset: false });
					renaming = false;
				};
			}}
			class="flex flex-wrap items-end gap-3"
		>
			<div class="min-w-56 flex-1 space-y-2">
				<Label for="new_name">New name</Label>
				<Input id="new_name" name="new_name" value={c.name} autocomplete="off" />
			</div>
			<Button type="submit" variant="outline" disabled={renaming}>
				{renaming ? 'Renaming…' : 'Rename'}
			</Button>
		</form>
		{#if form?.renameError}
			<p class="text-sm text-destructive">{form.renameError}</p>
		{/if}
	</section>

	<div class="mt-10 rounded-lg border border-destructive/40 p-5">
		<h2 class="text-sm font-medium text-destructive">Danger zone</h2>
		<p class="mt-1 text-sm text-muted-foreground">
			Deleting removes the cache and hides its paths. Stored data is retained but the cache is no
			longer reachable.
		</p>
		<form
			method="POST"
			action="?/delete"
			class="mt-4"
			use:enhance={({ cancel }) => {
				if (!confirm(`Delete cache "${c.name}"? Clients can no longer pull from it.`)) {
					cancel();
					return;
				}
				deleting = true;
				return async ({ update }) => {
					await update();
					deleting = false;
				};
			}}
		>
			<Button type="submit" variant="destructive" disabled={deleting}>
				<Trash2 class="size-4" />
				{deleting ? 'Deleting…' : 'Delete cache'}
			</Button>
		</form>
		{#if form?.deleteError}
			<p class="mt-3 text-sm text-destructive">{form.deleteError}</p>
		{/if}
	</div>
</div>
