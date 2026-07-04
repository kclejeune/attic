import { error, fail } from '@sveltejs/kit';
import { mintAtticToken, type CacheAccess, type CachePermission } from '$lib/server/attic-token';
import type { PageServerLoad, Actions } from './$types';

interface TokenRow {
	id: string;
	name: string;
	permissions: string;
	expires_at: number | null;
	revoked_at: number | null;
	created_at: number;
}

async function sha256hex(s: string): Promise<string> {
	const buf = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(s));
	return [...new Uint8Array(buf)].map((b) => b.toString(16).padStart(2, '0')).join('');
}

export const load: PageServerLoad = async ({ platform, locals }) => {
	if (!locals.user) throw error(401, 'Not signed in');
	const db = platform?.env.ATTIC_DB;
	if (!db) throw error(500, 'Database binding unavailable');

	const [{ results: tokens }, { results: caches }] = await Promise.all([
		db
			.prepare(
				`SELECT id, name, permissions, expires_at, revoked_at, created_at
				 FROM api_token WHERE user_id = ?1 ORDER BY created_at DESC`
			)
			.bind(locals.user.id)
			.all<TokenRow>(),
		db
			.prepare('SELECT name FROM cache WHERE deleted_at IS NULL ORDER BY name')
			.all<{ name: string }>()
	]);

	const now = Math.floor(Date.now() / 1000);
	return {
		cacheNames: caches.map((c) => c.name),
		tokens: tokens.map((t) => ({
			id: t.id,
			name: t.name,
			scope: t.permissions,
			createdAt: t.created_at,
			expiresAt: t.expires_at,
			status: t.revoked_at
				? ('revoked' as const)
				: t.expires_at && t.expires_at < now
					? ('expired' as const)
					: ('active' as const)
		}))
	};
};

export const actions: Actions = {
	issue: async ({ request, platform, locals }) => {
		if (!locals.user) throw error(401, 'Not signed in');
		const env = platform?.env;
		if (!env?.ATTIC_DB) throw error(500, 'Database binding unavailable');
		if (!env.JWT_HS256_SECRET_BASE64) {
			return fail(500, { error: 'Token signing is not configured (JWT_HS256_SECRET_BASE64).' });
		}

		const form = await request.formData();
		const name = String(form.get('name') ?? '').trim();
		const cacheScope = String(form.get('cache') ?? '*');
		const canPull = form.get('pull') === 'on';
		const canPush = form.get('push') === 'on';
		const days = Math.max(1, Math.min(3650, Number(form.get('expiry_days') ?? 90)));

		if (!name) return fail(400, { error: 'Give the token a name.' });
		if (!canPull && !canPush) return fail(400, { error: 'Grant at least one permission.' });

		const perm: CachePermission = {};
		if (canPull) perm.r = 1;
		if (canPush) perm.w = 1;
		const caches: CacheAccess = { [cacheScope]: perm };

		const jti = crypto.randomUUID();
		const ttl = days * 24 * 60 * 60;
		const token = await mintAtticToken(env.JWT_HS256_SECRET_BASE64, locals.user.id, caches, ttl, jti);

		const now = Math.floor(Date.now() / 1000);
		await env.ATTIC_DB.prepare(
			`INSERT INTO api_token (id, user_id, name, token_hash, permissions, expires_at, created_at)
			 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)`
		)
			.bind(
				jti,
				locals.user.id,
				name,
				await sha256hex(token),
				JSON.stringify(caches),
				now + ttl,
				now
			)
			.run();

		// The plaintext token is returned exactly once; only its hash is stored.
		return { issued: { name, token } };
	},

	revoke: async ({ request, platform, locals }) => {
		if (!locals.user) throw error(401, 'Not signed in');
		const db = platform?.env.ATTIC_DB;
		if (!db) throw error(500, 'Database binding unavailable');

		const id = String((await request.formData()).get('id') ?? '');
		await db
			.prepare('UPDATE api_token SET revoked_at = ?1 WHERE id = ?2 AND user_id = ?3')
			.bind(Math.floor(Date.now() / 1000), id, locals.user.id)
			.run();

		return { revoked: true };
	}
};
