import { error, fail, redirect } from '@sveltejs/kit';
import { mintAtticToken, type CacheAccess, type CachePermission } from '$lib/server/attic-token';
import type { PageServerLoad, Actions } from './$types';

function parsePort(raw: string | null): number | null {
	const port = Number(raw);
	return Number.isInteger(port) && port >= 1024 && port <= 65535 ? port : null;
}

export const load: PageServerLoad = async ({ url, locals, platform }) => {
	if (!locals.user) throw error(401, 'Not signed in');
	const db = platform?.env.ATTIC_DB;
	if (!db) throw error(500, 'Database binding unavailable');

	const port = parsePort(url.searchParams.get('port'));
	const state = url.searchParams.get('state') ?? '';
	if (port === null || !state) {
		throw error(400, 'Invalid CLI authorization request (missing port or state).');
	}

	const { results: caches } = await db
		.prepare('SELECT name FROM cache WHERE deleted_at IS NULL ORDER BY name')
		.all<{ name: string }>();

	return {
		port,
		state,
		label: url.searchParams.get('label') ?? 'attic CLI',
		hostname: url.searchParams.get('hostname') ?? '',
		cacheNames: caches.map((c) => c.name),
		user: { email: locals.user.email, name: locals.user.name }
	};
};

export const actions: Actions = {
	authorize: async ({ request, locals, platform }) => {
		if (!locals.user) throw error(401, 'Not signed in');
		const env = platform?.env;
		if (!env?.ATTIC_DB) throw error(500, 'Database binding unavailable');
		if (!env.JWT_HS256_SECRET_BASE64) {
			return fail(500, { error: 'Token signing is not configured.' });
		}

		const form = await request.formData();
		const port = parsePort(String(form.get('port')));
		const state = String(form.get('state') ?? '');
		if (port === null || !state) return fail(400, { error: 'Invalid request.' });

		const cacheScope = String(form.get('cache') ?? '*');
		const canPull = form.get('pull') === 'on';
		const canPush = form.get('push') === 'on';
		const label = String(form.get('label') ?? 'attic CLI').slice(0, 80);
		const days = Math.max(1, Math.min(3650, Number(form.get('expiry_days') ?? 90)));

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
		await env.ATTIC_DB.prepare(
			`INSERT INTO api_token (id, user_id, name, token_hash, permissions, expires_at, created_at)
			 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)`
		)
			.bind(jti, locals.user.id, label, hash, JSON.stringify(caches), now + ttl, now)
			.run();

		// Hand the token back to the CLI's loopback listener. The host is fixed to
		// 127.0.0.1 (only the port is caller-supplied), so there is no open-redirect.
		const target = `http://127.0.0.1:${port}/callback?token=${encodeURIComponent(token)}&state=${encodeURIComponent(state)}`;
		redirect(303, target);
	}
};
