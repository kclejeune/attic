import { dev } from '$app/environment';
import { error } from '@sveltejs/kit';
import type { PageServerLoad } from './$types';

interface WeekRow {
	week_start: string;
	paths: number;
	bytes: number;
}

export interface Bucket {
	/** ISO date of the week's start (Monday-ish; sqlite %W week). */
	weekStart: string;
	paths: number;
	/** Bytes added this week. */
	bytes: number;
	/** Cumulative bytes through this week. */
	cumulativeBytes: number;
}

export const load: PageServerLoad = async ({ platform }) => {
	const db = platform?.env.ATTIC_DB;

	// Local preview without bindings: synthesize a plausible series.
	if (!db) {
		if (!dev) throw error(500, 'Database binding unavailable');
		return { buckets: sampleBuckets() };
	}

	const { results } = await db
		.prepare(
			`SELECT MIN(date(o.created_at)) AS week_start,
			        COUNT(*) AS paths,
			        COALESCE(SUM(ch.file_size), 0) AS bytes
			 FROM object o
			 JOIN nar n ON n.id = o.nar_id
			 JOIN chunkref cr ON cr.nar_id = n.id
			 JOIN chunk ch ON ch.id = cr.chunk_id
			 GROUP BY strftime('%Y-%W', o.created_at)
			 ORDER BY week_start`
		)
		.all<WeekRow>();

	let cumulative = 0;
	const buckets: Bucket[] = results.map((r) => {
		cumulative += r.bytes;
		return { weekStart: r.week_start, paths: r.paths, bytes: r.bytes, cumulativeBytes: cumulative };
	});

	return { buckets };
};

function sampleBuckets(): Bucket[] {
	const out: Bucket[] = [];
	let cumulative = 0;
	const start = Date.UTC(2026, 0, 5);
	for (let i = 0; i < 20; i++) {
		const paths = Math.round(20 + 60 * Math.abs(Math.sin(i / 2)) + (i % 3) * 12);
		const bytes = paths * (4_000_000 + (i % 4) * 1_500_000);
		cumulative += bytes;
		out.push({
			weekStart: new Date(start + i * 7 * 86400_000).toISOString().slice(0, 10),
			paths,
			bytes,
			cumulativeBytes: cumulative
		});
	}
	return out;
}
