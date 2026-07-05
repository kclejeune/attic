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

	// Bucket by the Monday of each week (UTC) so the series has a canonical,
	// gap-fillable key rather than "first date seen in the week".
	const { results } = await db
		.prepare(
			`SELECT date(o.created_at, '-' || ((cast(strftime('%w', o.created_at) AS INTEGER) + 6) % 7) || ' days') AS week_start,
			        COUNT(*) AS paths,
			        COALESCE(SUM(ch.file_size), 0) AS bytes
			 FROM object o
			 JOIN nar n ON n.id = o.nar_id
			 JOIN chunkref cr ON cr.nar_id = n.id
			 JOIN chunk ch ON ch.id = cr.chunk_id
			 GROUP BY week_start
			 ORDER BY week_start`
		)
		.all<WeekRow>();

	return { buckets: fillWeeks(results) };
}

const WEEK_MS = 7 * 86400_000;

/** Monday (UTC) of the week containing `ms`, as an epoch-ms value. */
function mondayOf(ms: number): number {
	const d = new Date(ms);
	const daysSinceMonday = (d.getUTCDay() + 6) % 7;
	return Date.UTC(d.getUTCFullYear(), d.getUTCMonth(), d.getUTCDate() - daysSinceMonday);
}

/**
 * Turn sparse weekly rows into a continuous week-by-week series from the first
 * active week through the current week, zero-filling weeks with no uploads and
 * carrying cumulative storage forward across them.
 */
function fillWeeks(rows: WeekRow[]): Bucket[] {
	if (rows.length === 0) return [];

	const byWeek = new Map(rows.map((r) => [r.week_start, r]));
	const startMs = Date.parse(`${rows[0].week_start}T00:00:00Z`);
	const lastMs = Date.parse(`${rows[rows.length - 1].week_start}T00:00:00Z`);
	const endMs = Math.max(lastMs, mondayOf(Date.now()));

	const buckets: Bucket[] = [];
	let cumulative = 0;
	// Cap iterations as a guard against unexpected date values.
	for (let ms = startMs, i = 0; ms <= endMs && i < 520; ms += WEEK_MS, i++) {
		const weekStart = new Date(ms).toISOString().slice(0, 10);
		const row = byWeek.get(weekStart);
		const bytes = row?.bytes ?? 0;
		cumulative += bytes;
		buckets.push({ weekStart, paths: row?.paths ?? 0, bytes, cumulativeBytes: cumulative });
	}
	return buckets;
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
