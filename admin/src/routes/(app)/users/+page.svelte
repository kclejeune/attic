<script lang="ts">
	import { invalidateAll } from '$app/navigation';
	import * as DropdownMenu from '$lib/components/ui/dropdown-menu/index.js';
	import { ShieldCheck, MoreHorizontal } from '@lucide/svelte';

	let { data } = $props();
	let error = $state('');

	async function setRole(userId: string, role: 'admin' | 'member') {
		error = '';
		const body = new FormData();
		body.set('userId', userId);
		body.set('role', role);
		const res = await fetch('?/setRole', { method: 'POST', body });
		if (!res.ok) {
			error = 'Failed to update role.';
			return;
		}
		await invalidateAll();
	}
</script>

<div class="mx-auto max-w-6xl px-8 py-8">
	<header class="mb-8">
		<h1 class="text-2xl font-semibold tracking-tight">Users</h1>
		<p class="mt-1 text-sm text-muted-foreground">
			Everyone who signs in appears here. Admins can manage caches, tokens, and other users.
		</p>
	</header>

	{#if error}
		<p class="mb-4 text-sm text-destructive">{error}</p>
	{/if}

	<div class="overflow-hidden rounded-lg border">
		<table class="w-full text-sm">
			<thead class="border-b bg-muted/40 text-left text-xs text-muted-foreground">
				<tr>
					<th class="px-4 py-2.5 font-medium">User</th>
					<th class="px-4 py-2.5 font-medium">Sign-in</th>
					<th class="px-4 py-2.5 font-medium">Role</th>
					<th class="w-12 px-4 py-2.5"></th>
				</tr>
			</thead>
			<tbody class="divide-y">
				{#each data.users as u (u.id)}
					<tr class="transition-colors hover:bg-muted/30">
						<td class="px-4 py-3">
							<div class="font-medium">{u.name}</div>
							<div class="font-mono text-xs text-muted-foreground">{u.email}</div>
						</td>
						<td class="px-4 py-3 text-muted-foreground">{u.provider}</td>
						<td class="px-4 py-3">
							{#if u.role === 'admin'}
								<span class="inline-flex items-center gap-1.5 font-medium text-primary">
									<ShieldCheck class="size-3.5" /> Admin
								</span>
							{:else}
								<span class="text-muted-foreground">Member</span>
							{/if}
						</td>
						<td class="px-4 py-3 text-right">
							<DropdownMenu.Root>
								<DropdownMenu.Trigger
									class="inline-flex size-8 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-accent hover:text-accent-foreground"
									aria-label="Manage user"
								>
									<MoreHorizontal class="size-4" />
								</DropdownMenu.Trigger>
								<DropdownMenu.Content align="end">
									<DropdownMenu.Item
										disabled={u.role === 'admin'}
										onSelect={() => setRole(u.id, 'admin')}
									>
										Make admin
									</DropdownMenu.Item>
									<DropdownMenu.Item
										disabled={u.role === 'member'}
										onSelect={() => setRole(u.id, 'member')}
									>
										Make member
									</DropdownMenu.Item>
								</DropdownMenu.Content>
							</DropdownMenu.Root>
						</td>
					</tr>
				{/each}
			</tbody>
		</table>
	</div>
</div>
