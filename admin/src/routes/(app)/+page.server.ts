import { error } from '@sveltejs/kit';
import type { PageServerLoad } from './$types';

type Count = { n: number };

export const load: PageServerLoad = async ({ platform }) => {
	const db = platform?.env.ATTIC_DB;
	if (!db) throw error(500, 'Database binding unavailable');

	const [caches, objects, nars, storage, pending] = await Promise.all([
		db.prepare('SELECT COUNT(*) AS n FROM cache WHERE deleted_at IS NULL').first<Count>(),
		db.prepare('SELECT COUNT(*) AS n FROM object').first<Count>(),
		db.prepare("SELECT COUNT(*) AS n FROM nar WHERE state = 'V'").first<Count>(),
		db.prepare("SELECT COALESCE(SUM(file_size), 0) AS n FROM chunk WHERE state = 'V'").first<Count>(),
		db.prepare("SELECT COUNT(*) AS n FROM nar WHERE state = 'P'").first<Count>()
	]);

	return {
		stats: {
			caches: caches?.n ?? 0,
			objects: objects?.n ?? 0,
			nars: nars?.n ?? 0,
			storageBytes: storage?.n ?? 0,
			pendingNars: pending?.n ?? 0
		}
	};
};
