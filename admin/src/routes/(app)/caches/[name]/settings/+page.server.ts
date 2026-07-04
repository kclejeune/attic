import { error, fail, redirect } from '@sveltejs/kit';
import { atticFetch, adminAccess } from '$lib/server/attic-api';
import type { PageServerLoad, Actions } from './$types';

const CACHE_NAME = /^[a-z0-9][a-z0-9-]{0,49}$/;

interface CacheRow {
	name: string;
	is_public: number;
	priority: number;
	compression: string;
	retention_period: number | null;
}

export const load: PageServerLoad = async ({ platform, params }) => {
	const db = platform?.env.ATTIC_DB;
	if (!db) throw error(500, 'Database binding unavailable');

	const cache = await db
		.prepare(
			`SELECT name, is_public, priority, compression, retention_period
			 FROM cache WHERE name = ?1 AND deleted_at IS NULL`
		)
		.bind(params.name)
		.first<CacheRow>();

	if (!cache) throw error(404, `Cache "${params.name}" not found`);

	return {
		cache: {
			name: cache.name,
			isPublic: cache.is_public !== 0,
			priority: cache.priority,
			compression: cache.compression,
			retentionDays: cache.retention_period
		}
	};
};

export const actions: Actions = {
	save: async ({ request, locals, platform, params }) => {
		if (!locals.user) throw error(401, 'Not signed in');
		if (!platform?.env) throw error(500, 'Platform bindings unavailable');

		const form = await request.formData();
		const isPublic = form.get('is_public') === 'on';
		const priority = Number(form.get('priority') ?? 40);
		const compression = String(form.get('compression') ?? 'zstd');
		const retentionRaw = String(form.get('retention_period') ?? '').trim();
		const retention = retentionRaw === '' ? null : Number(retentionRaw);

		const res = await atticFetch(
			platform.env,
			{ userId: locals.user.id, caches: adminAccess() },
			`/_api/v1/cache-config/${encodeURIComponent(params.name)}`,
			{
				method: 'PATCH',
				headers: { 'content-type': 'application/json' },
				body: JSON.stringify({
					is_public: isPublic,
					priority,
					compression,
					retention_period: retention
				})
			}
		);

		if (!res.ok) {
			return fail(502, { error: `Failed to save: ${await res.text()}` });
		}

		return { saved: true };
	},

	rename: async ({ request, locals, platform, params }) => {
		if (!locals.user) throw error(401, 'Not signed in');
		if (!platform?.env) throw error(500, 'Platform bindings unavailable');

		const newName = String((await request.formData()).get('new_name') ?? '').trim();

		if (!CACHE_NAME.test(newName)) {
			return fail(400, {
				renameError: 'Name must be lowercase alphanumeric with dashes (max 50 chars).'
			});
		}
		if (newName === params.name) {
			return fail(400, { renameError: 'That is already the cache name.' });
		}

		const res = await atticFetch(
			platform.env,
			{ userId: locals.user.id, caches: adminAccess() },
			`/_api/v1/cache-config/${encodeURIComponent(params.name)}/rename`,
			{
				method: 'POST',
				headers: { 'content-type': 'application/json' },
				body: JSON.stringify({ new_name: newName })
			}
		);

		if (!res.ok) {
			const detail = await res.text();
			return fail(res.status === 409 ? 409 : 502, {
				renameError:
					res.status === 409
						? `A cache named "${newName}" already exists.`
						: `Failed to rename: ${detail}`
			});
		}

		redirect(303, `/caches/${newName}/settings`);
	},

	delete: async ({ locals, platform, params }) => {
		if (!locals.user) throw error(401, 'Not signed in');
		if (!platform?.env) throw error(500, 'Platform bindings unavailable');

		const res = await atticFetch(
			platform.env,
			{ userId: locals.user.id, caches: adminAccess() },
			`/_api/v1/cache-config/${encodeURIComponent(params.name)}`,
			{ method: 'DELETE' }
		);

		if (!res.ok) {
			return fail(502, { deleteError: `Failed to delete: ${await res.text()}` });
		}

		redirect(303, '/caches');
	}
};
