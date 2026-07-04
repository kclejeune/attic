import { error } from '@sveltejs/kit';
import type { PageServerLoad } from './$types';

const PAGE_SIZE = 50;

interface CacheRow {
	name: string;
	is_public: number;
	priority: number;
	compression: string;
	retention_period: number | null;
	store_dir: string;
}

interface PathRow {
	store_path: string;
	store_path_hash: string;
	nar_size: number;
	created_at: string;
}

export const load: PageServerLoad = async ({ platform, params, url }) => {
	const db = platform?.env.ATTIC_DB;
	if (!db) throw error(500, 'Database binding unavailable');

	const page = Math.max(0, Number(url.searchParams.get('page') ?? '0'));
	const offset = page * PAGE_SIZE;

	const cache = await db
		.prepare(
			`SELECT name, is_public, priority, compression, retention_period, store_dir
			 FROM cache WHERE name = ?1 AND deleted_at IS NULL`
		)
		.bind(params.name)
		.first<CacheRow>();

	if (!cache) throw error(404, `Cache "${params.name}" not found`);

	const [{ results: paths }, totals] = await Promise.all([
		db
			.prepare(
				`SELECT o.store_path, o.store_path_hash, n.nar_size, o.created_at
				 FROM object o
				 JOIN cache c ON c.id = o.cache_id
				 JOIN nar n ON n.id = o.nar_id
				 WHERE c.name = ?1
				 ORDER BY o.created_at DESC
				 LIMIT ?2 OFFSET ?3`
			)
			.bind(params.name, PAGE_SIZE, offset)
			.all<PathRow>(),
		db
			.prepare(
				`SELECT COUNT(*) AS n FROM object o JOIN cache c ON c.id = o.cache_id WHERE c.name = ?1`
			)
			.bind(params.name)
			.first<{ n: number }>()
	]);

	const total = totals?.n ?? 0;

	return {
		cache: {
			name: cache.name,
			isPublic: cache.is_public !== 0,
			priority: cache.priority,
			compression: cache.compression,
			retentionDays: cache.retention_period,
			storeDir: cache.store_dir
		},
		paths: paths.map((p) => ({
			storePath: p.store_path,
			hash: p.store_path_hash,
			narSize: p.nar_size,
			createdAt: p.created_at
		})),
		page,
		pageSize: PAGE_SIZE,
		total,
		hasMore: offset + paths.length < total
	};
};
