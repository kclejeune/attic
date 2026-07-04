import { error, fail } from '@sveltejs/kit';
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

export const load: PageServerLoad = async ({ platform, locals }) => {
	requireAdmin(locals);
	const db = platform?.env.ATTIC_DB;
	if (!db) throw error(500, 'Database binding unavailable');

	const { results } = await db
		.prepare('SELECT id, name, email, role, createdAt FROM user ORDER BY createdAt')
		.all<UserRow>();

	return {
		users: results.map((u) => ({
			id: u.id,
			name: u.name,
			email: u.email,
			role: u.role,
			provider: u.id.startsWith('cfaccess:') ? 'Cloudflare Access' : 'OIDC',
			createdAt: u.createdAt
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
	}
};
