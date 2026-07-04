<script lang="ts">
	import { enhance } from '$app/forms';
	import { ShieldCheck } from '@lucide/svelte';

	let { data, form } = $props();
</script>

<div class="mx-auto max-w-6xl px-8 py-8">
	<header class="mb-8">
		<h1 class="text-2xl font-semibold tracking-tight">Users</h1>
		<p class="mt-1 text-sm text-muted-foreground">
			Everyone who signs in appears here. Admins can manage caches, tokens, and other users.
		</p>
	</header>

	{#if form?.error}
		<p class="mb-4 text-sm text-destructive">{form.error}</p>
	{/if}

	<div class="overflow-hidden rounded-lg border">
		<table class="w-full text-sm">
			<thead class="border-b bg-muted/40 text-left text-xs text-muted-foreground">
				<tr>
					<th class="px-4 py-2.5 font-medium">User</th>
					<th class="px-4 py-2.5 font-medium">Sign-in</th>
					<th class="px-4 py-2.5 font-medium">Role</th>
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
							<form method="POST" action="?/setRole" use:enhance class="flex items-center gap-2">
								<input type="hidden" name="userId" value={u.id} />
								{#if u.role === 'admin'}
									<span
										class="inline-flex items-center gap-1.5 text-sm font-medium text-primary"
									>
										<ShieldCheck class="size-3.5" /> Admin
									</span>
								{/if}
								<select
									name="role"
									value={u.role}
									onchange={(e) => e.currentTarget.form?.requestSubmit()}
									class="h-8 rounded-md border border-input bg-transparent px-2 text-sm focus-visible:ring-1 focus-visible:ring-ring focus-visible:outline-none"
								>
									<option value="member">member</option>
									<option value="admin">admin</option>
								</select>
							</form>
						</td>
					</tr>
				{/each}
			</tbody>
		</table>
	</div>
</div>
