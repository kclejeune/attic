import { error, fail } from '@sveltejs/kit';
import { atticFetch, adminAccess } from '$lib/server/attic-api';
import type { PageServerLoad, Actions } from './$types';

type Count = { n: number };

export const load: PageServerLoad = async ({ platform }) => {
	const db = platform?.env.ATTIC_DB;
	if (!db) throw error(500, 'Database binding unavailable');

	const [caches, objects, nars, storage, pending, orphanNars, orphanChunks] = await Promise.all([
		db.prepare('SELECT COUNT(*) AS n FROM cache WHERE deleted_at IS NULL').first<Count>(),
		db.prepare('SELECT COUNT(*) AS n FROM object').first<Count>(),
		db.prepare("SELECT COUNT(*) AS n FROM nar WHERE state = 'V'").first<Count>(),
		db.prepare("SELECT COALESCE(SUM(file_size), 0) AS n FROM chunk WHERE state = 'V'").first<Count>(),
		db.prepare("SELECT COUNT(*) AS n FROM nar WHERE state = 'P'").first<Count>(),
		db
			.prepare('SELECT COUNT(*) AS n FROM nar n WHERE NOT EXISTS (SELECT 1 FROM object o WHERE o.nar_id = n.id)')
			.first<Count>(),
		db
			.prepare('SELECT COUNT(*) AS n FROM chunk WHERE NOT EXISTS (SELECT 1 FROM chunkref cr WHERE cr.chunk_id = chunk.id)')
			.first<Count>()
	]);

	return {
		stats: {
			caches: caches?.n ?? 0,
			objects: objects?.n ?? 0,
			nars: nars?.n ?? 0,
			storageBytes: storage?.n ?? 0,
			pendingNars: pending?.n ?? 0,
			orphanNars: orphanNars?.n ?? 0,
			orphanChunks: orphanChunks?.n ?? 0
		}
	};
};

export const actions: Actions = {
	gc: async ({ locals, platform }) => {
		if (!locals.user) throw error(401, 'Not signed in');
		if (locals.user.role !== 'admin') throw error(403, 'Admins only');
		if (!platform?.env) throw error(500, 'Platform bindings unavailable');

		const res = await atticFetch(
			platform.env,
			{ userId: locals.user.id, caches: adminAccess() },
			'/_api/v1/gc',
			{ method: 'POST' }
		);
		if (!res.ok) {
			return fail(502, { gcError: `Garbage collection failed: ${await res.text()}` });
		}
		return { gcStats: (await res.json()) as Record<string, number> };
	}
};
