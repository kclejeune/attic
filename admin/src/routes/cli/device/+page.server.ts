import { error, fail } from '@sveltejs/kit';
import { mintAtticToken, type CacheAccess, type CachePermission } from '$lib/server/attic-token';
import type { PageServerLoad, Actions } from './$types';

interface GrantRow {
	user_code: string;
	status: string;
	expires_at: number;
}

export const load: PageServerLoad = async ({ url, locals, platform }) => {
	if (!locals.user) throw error(401, 'Not signed in');
	const db = platform?.env.ATTIC_DB;
	if (!db) throw error(500, 'Database binding unavailable');

	const code = (url.searchParams.get('code') ?? '').trim().toUpperCase();

	let grant: { userCode: string; status: string; expired: boolean } | null = null;
	if (code) {
		const row = await db
			.prepare('SELECT user_code, status, expires_at FROM device_auth WHERE user_code = ?1')
			.bind(code)
			.first<GrantRow>();
		if (row) {
			grant = {
				userCode: row.user_code,
				status: row.status,
				expired: row.expires_at < Math.floor(Date.now() / 1000)
			};
		}
	}

	const { results: caches } = await db
		.prepare('SELECT name FROM cache WHERE deleted_at IS NULL ORDER BY name')
		.all<{ name: string }>();

	return {
		code,
		grant,
		notFound: Boolean(code) && grant === null,
		cacheNames: caches.map((c) => c.name),
		user: { email: locals.user.email, name: locals.user.name }
	};
};

export const actions: Actions = {
	approve: async ({ request, locals, platform }) => {
		if (!locals.user) throw error(401, 'Not signed in');
		const env = platform?.env;
		if (!env?.ATTIC_DB) throw error(500, 'Database binding unavailable');
		if (!env.JWT_HS256_SECRET_BASE64) return fail(500, { error: 'Token signing not configured.' });

		const form = await request.formData();
		const userCode = String(form.get('user_code') ?? '').trim().toUpperCase();
		const cacheScope = String(form.get('cache') ?? '*');
		const canPull = form.get('pull') === 'on';
		const canPush = form.get('push') === 'on';
		const label = String(form.get('label') ?? 'attic CLI').slice(0, 80);
		const days = Math.max(1, Math.min(3650, Number(form.get('expiry_days') ?? 90)));

		const row = await env.ATTIC_DB.prepare(
			'SELECT status, expires_at FROM device_auth WHERE user_code = ?1'
		)
			.bind(userCode)
			.first<{ status: string; expires_at: number }>();
		if (!row) return fail(404, { error: 'Unknown code — check for typos.' });
		if (row.status !== 'pending') return fail(400, { error: 'This code was already used.' });
		if (row.expires_at < Math.floor(Date.now() / 1000)) {
			return fail(400, { error: 'This code has expired — start login again.' });
		}
		if (!canPull && !canPush) return fail(400, { error: 'Grant at least one permission.' });

		const perm: CachePermission = {};
		if (canPull) perm.r = 1;
		if (canPush) perm.w = 1;
		const caches: CacheAccess = { [cacheScope]: perm };

		const jti = crypto.randomUUID();
		const ttl = days * 24 * 60 * 60;
		const token = await mintAtticToken(env.JWT_HS256_SECRET_BASE64, locals.user.id, caches, ttl, jti);
		const now = Math.floor(Date.now() / 1000);

		const buf = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(token));
		const hash = [...new Uint8Array(buf)].map((b) => b.toString(16).padStart(2, '0')).join('');

		await env.ATTIC_DB.batch([
			env.ATTIC_DB.prepare(
				`INSERT INTO api_token (id, user_id, name, token_hash, permissions, expires_at, created_at)
				 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)`
			).bind(jti, locals.user.id, label, hash, JSON.stringify(caches), now + ttl, now),
			env.ATTIC_DB.prepare(
				`UPDATE device_auth SET status = 'approved', scope = ?1, user_id = ?2, token = ?3
				 WHERE user_code = ?4 AND status = 'pending'`
			).bind(JSON.stringify(caches), locals.user.id, token, userCode)
		]);

		return { approved: true };
	}
};
