<script lang="ts">
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import { authClient } from '$lib/auth-client';
	import { LayoutDashboard, Boxes, KeyRound, LogOut } from '@lucide/svelte';

	let { children, data } = $props();

	const nav = [
		{ href: '/', label: 'Overview', icon: LayoutDashboard },
		{ href: '/caches', label: 'Caches', icon: Boxes },
		{ href: '/tokens', label: 'Tokens', icon: KeyRound }
	];

	function isActive(href: string): boolean {
		return href === '/' ? page.url.pathname === '/' : page.url.pathname.startsWith(href);
	}

	async function signOut() {
		await authClient.signOut();
		await goto('/login');
	}

	const user = $derived(data.user);
	const initial = $derived((user?.name ?? user?.email ?? '?').slice(0, 1).toUpperCase());
</script>

<div class="grid min-h-svh grid-cols-[15rem_1fr] bg-background text-foreground">
	<aside class="flex flex-col border-r border-sidebar-border bg-sidebar">
		<div class="flex items-center gap-2.5 px-5 py-5">
			<!-- Crystalline mark: a nod to the Nix snowflake -->
			<svg viewBox="0 0 24 24" class="size-6 text-primary" aria-hidden="true">
				<path
					fill="currentColor"
					d="M12 1.5 8.5 7.5H2l3.25 5.62L2 18.75h6.5L12 24.75l3.5-6h6.5l-3.25-5.63L22 7.5h-6.5L12 1.5Zm0 4.2 1.9 3.3H10.1L12 5.7Zm-6.6 3.3h3.8L7.3 12.3 5.4 9Zm9.4 0h3.8L16.7 12.3 14.8 9Zm-4.7 3.3h3.8L12 18.6l-1.9-3.3Z"
				/>
			</svg>
			<span class="font-mono text-lg font-semibold tracking-tight">attic</span>
		</div>

		<nav class="flex flex-1 flex-col gap-0.5 px-3 py-2">
			{#each nav as item (item.href)}
				{@const Icon = item.icon}
				<a
					href={item.href}
					class="flex items-center gap-3 rounded-md px-3 py-2 text-sm font-medium transition-colors
						{isActive(item.href)
						? 'bg-sidebar-accent text-sidebar-accent-foreground'
						: 'text-muted-foreground hover:bg-sidebar-accent/60 hover:text-sidebar-foreground'}"
					aria-current={isActive(item.href) ? 'page' : undefined}
				>
					<Icon class="size-4" />
					{item.label}
				</a>
			{/each}
		</nav>

		<div class="border-t border-sidebar-border p-3">
			<div class="flex items-center gap-2.5 px-2 py-1.5">
				<div
					class="flex size-8 shrink-0 items-center justify-center rounded-full bg-primary text-sm font-semibold text-primary-foreground"
				>
					{initial}
				</div>
				<div class="min-w-0 flex-1">
					<div class="truncate text-sm font-medium">{user?.name ?? user?.email ?? 'You'}</div>
					<div class="truncate text-xs text-muted-foreground">{user?.email ?? ''}</div>
				</div>
				<button
					onclick={signOut}
					title="Sign out"
					class="rounded-md p-1.5 text-muted-foreground transition-colors hover:bg-sidebar-accent hover:text-sidebar-foreground"
				>
					<LogOut class="size-4" />
				</button>
			</div>
		</div>
	</aside>

	<main class="min-w-0 overflow-x-hidden">
		{@render children()}
	</main>
</div>
