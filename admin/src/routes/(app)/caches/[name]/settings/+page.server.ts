import { error, fail } from '@sveltejs/kit';
import { atticFetch, adminAccess } from '$lib/server/attic-api';
import type { PageServerLoad, Actions } from './$types';

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
	default: async ({ request, locals, platform, params }) => {
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
	}
};
