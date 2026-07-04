import { error, fail } from '@sveltejs/kit';
import type { D1Database } from '@cloudflare/workers-types';
import type { PageServerLoad, Actions } from './$types';

interface UserRow {
	id: string;
	name: string;
	email: string;
	role: string;
	createdAt: number;
}

function requireAdmin(locals: App.Locals) {
	if (!locals.user) throw error(401, 'Not signed in');
	if (locals.user.role !== 'admin') throw error(403, 'Admins only');
}

/** The protected owner is the first account created. */
async function ownerId(db: D1Database): Promise<string | null> {
	const row = await db
		.prepare('SELECT id FROM user ORDER BY createdAt LIMIT 1')
		.first<{ id: string }>();
	return row?.id ?? null;
}

export const load: PageServerLoad = async ({ platform, locals }) => {
	requireAdmin(locals);
	const db = platform?.env.ATTIC_DB;
	if (!db) throw error(500, 'Database binding unavailable');

	const { results } = await db
		.prepare('SELECT id, name, email, role, createdAt FROM user ORDER BY createdAt')
		.all<UserRow>();

	return {
		currentUserId: locals.user!.id,
		users: results.map((u, i) => ({
			id: u.id,
			name: u.name,
			email: u.email,
			role: u.role,
			provider: u.id.startsWith('cfaccess:') ? 'Cloudflare Access' : 'OIDC',
			createdAt: u.createdAt,
			isOwner: i === 0
		}))
	};
};

export const actions: Actions = {
	setRole: async ({ request, platform, locals }) => {
		requireAdmin(locals);
		const db = platform?.env.ATTIC_DB;
		if (!db) throw error(500, 'Database binding unavailable');

		const form = await request.formData();
		const userId = String(form.get('userId') ?? '');
		const role = String(form.get('role') ?? '');
		if (role !== 'admin' && role !== 'member') return fail(400, { error: 'Invalid role' });

		if (userId === locals.user!.id && role !== 'admin') {
			return fail(400, { error: 'You cannot remove your own admin role.' });
		}

		await db
			.prepare('UPDATE user SET role = ?1, updatedAt = ?2 WHERE id = ?3')
			.bind(role, Math.floor(Date.now() / 1000), userId)
			.run();

		return { saved: true };
	},

	addUser: async ({ request, platform, locals }) => {
		requireAdmin(locals);
		const db = platform?.env.ATTIC_DB;
		if (!db) throw error(500, 'Database binding unavailable');

		const form = await request.formData();
		const email = String(form.get('email') ?? '')
			.trim()
			.toLowerCase();
		const role = form.get('role') === 'admin' ? 'admin' : 'member';

		if (!/^[^@\s]+@[^@\s]+\.[^@\s]+$/.test(email)) {
			return fail(400, { error: 'Enter a valid email address.' });
		}

		const existing = await db
			.prepare('SELECT id FROM user WHERE email = ?1')
			.bind(email)
			.first<{ id: string }>();
		if (existing) return fail(400, { error: 'A user with that email already exists.' });

		// Pre-provision the account; it adopts the assigned role on first sign-in
		// (the Cloudflare Access path matches by email).
		const now = Math.floor(Date.now() / 1000);
		await db
			.prepare(
				`INSERT INTO user (id, name, email, emailVerified, role, createdAt, updatedAt)
				 VALUES (?1, ?2, ?3, 1, ?4, ?5, ?5)`
			)
			.bind(crypto.randomUUID(), email, email, role, now)
			.run();

		return { added: email };
	},

	deleteUser: async ({ request, platform, locals }) => {
		requireAdmin(locals);
		const db = platform?.env.ATTIC_DB;
		if (!db) throw error(500, 'Database binding unavailable');

		const userId = String((await request.formData()).get('userId') ?? '');

		if (userId === locals.user!.id) {
			return fail(400, { error: 'You cannot delete your own account.' });
		}
		if (userId === (await ownerId(db))) {
			return fail(400, { error: 'The owner account cannot be deleted.' });
		}

		// D1 does not enforce foreign keys, so clean up dependents explicitly.
		await db.batch([
			db.prepare('DELETE FROM api_token WHERE user_id = ?1').bind(userId),
			db.prepare('DELETE FROM session WHERE userId = ?1').bind(userId),
			db.prepare('DELETE FROM account WHERE userId = ?1').bind(userId),
			db.prepare('DELETE FROM user WHERE id = ?1').bind(userId)
		]);

		return { deleted: true };
	}
};
