import { error } from '@sveltejs/kit';
import {
	PATHS_PAGE_SIZE,
	parseSort,
	parseDir,
	queryStorePaths,
	countStorePaths
} from '$lib/server/store-paths';
import type { PageServerLoad } from './$types';

interface CacheRow {
	name: string;
	is_public: number;
	priority: number;
	compression: string;
	retention_period: number | null;
	store_dir: string;
	keypair: string;
}

/** Keypair is stored as `{name}:{base64(secret32 || public32)}`; return the Nix trusted-key form. */
function derivePublicKey(keypair: string): string | null {
	const idx = keypair.indexOf(':');
	if (idx < 0) return null;
	const name = keypair.slice(0, idx);
	try {
		const raw = Uint8Array.from(atob(keypair.slice(idx + 1)), (c) => c.charCodeAt(0));
		if (raw.length < 64) return null;
		let bin = '';
		for (const b of raw.slice(32, 64)) bin += String.fromCharCode(b);
		return `${name}:${btoa(bin)}`;
	} catch {
		return null;
	}
}

export const load: PageServerLoad = async ({ platform, params, url }) => {
	const db = platform?.env.ATTIC_DB;
	if (!db) throw error(500, 'Database binding unavailable');

	const sort = parseSort(url.searchParams.get('sort'));
	const dir = parseDir(url.searchParams.get('dir'));
	const q = (url.searchParams.get('q') ?? '').trim();

	const cache = await db
		.prepare(
			`SELECT name, is_public, priority, compression, retention_period, store_dir, keypair
			 FROM cache WHERE name = ?1 AND deleted_at IS NULL`
		)
		.bind(params.name)
		.first<CacheRow>();

	if (!cache) throw error(404, `Cache "${params.name}" not found`);

	const cacheBase = (platform?.env.CACHE_BASE_URL ?? 'https://cache.kclj.io').replace(/\/$/, '');
	const publicKey = derivePublicKey(cache.keypair);

	const [{ paths, hasMore }, total] = await Promise.all([
		queryStorePaths(db, params.name, { sort, dir, q, limit: PATHS_PAGE_SIZE, offset: 0 }),
		countStorePaths(db, params.name, q)
	]);

	return {
		cache: {
			name: cache.name,
			isPublic: cache.is_public !== 0,
			priority: cache.priority,
			compression: cache.compression,
			retentionDays: cache.retention_period,
			storeDir: cache.store_dir,
			url: `${cacheBase}/${cache.name}`,
			publicKey
		},
		paths,
		hasMore,
		total,
		sort,
		dir,
		q
	};
};
