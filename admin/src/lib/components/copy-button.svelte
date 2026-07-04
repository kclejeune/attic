<script lang="ts">
	import { Copy, Check } from '@lucide/svelte';

	let { text, label = 'Copy', class: className = '' }: { text: string; label?: string; class?: string } =
		$props();

	let copied = $state(false);

	async function copy() {
		try {
			await navigator.clipboard.writeText(text);
			copied = true;
			setTimeout(() => (copied = false), 1500);
		} catch {
			copied = false;
		}
	}
</script>

<button
	type="button"
	onclick={copy}
	title={label}
	aria-label={label}
	class="inline-flex size-8 shrink-0 items-center justify-center rounded-md border border-input text-muted-foreground transition-colors hover:bg-accent hover:text-accent-foreground {className}"
>
	{#if copied}
		<Check class="size-4 text-primary" />
	{:else}
		<Copy class="size-4" />
	{/if}
</button>
