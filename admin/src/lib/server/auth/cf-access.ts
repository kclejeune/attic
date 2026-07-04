import { createRemoteJWKSet, jwtVerify, type JWTPayload } from 'jose';
import { eq } from 'drizzle-orm';
import type { RequestEvent } from '@sveltejs/kit';
import { getDb, schema } from '$lib/server/db';
import type { SessionUser } from './types';

type Env = App.Platform['env'];

// Cache one JWKS resolver per team domain across requests.
const jwksCache = new Map<string, ReturnType<typeof createRemoteJWKSet>>();

function getJwks(teamDomain: string) {
	let jwks = jwksCache.get(teamDomain);
	if (!jwks) {
		jwks = createRemoteJWKSet(new URL(`${teamDomain.replace(/\/$/, '')}/cdn-cgi/access/certs`));
		jwksCache.set(teamDomain, jwks);
	}
	return jwks;
}

/**
 * If the request carries a valid Cloudflare Access assertion, return the
 * corresponding user (creating one on first sight). Returns null when Access is
 * not configured or the assertion is absent/invalid.
 *
 * Access re-validates on every request, so no local session is needed for this
 * path — identity is derived per request from the signed assertion.
 */
export async function resolveCfAccessUser(
	event: RequestEvent,
	env: Env
): Promise<SessionUser | null> {
	const teamDomain = env.CF_ACCESS_TEAM_DOMAIN;
	const aud = env.CF_ACCESS_AUD;
	if (!teamDomain || !aud) return null;

	const token =
		event.request.headers.get('Cf-Access-Jwt-Assertion') ??
		event.cookies.get('CF_Authorization');
	if (!token) return null;

	let payload: JWTPayload & { email?: string; name?: string };
	try {
		const result = await jwtVerify(token, getJwks(teamDomain), {
			issuer: teamDomain.replace(/\/$/, ''),
			audience: aud
		});
		payload = result.payload;
	} catch {
		return null;
	}

	const sub = payload.sub;
	const email = payload.email ?? null;
	if (!sub) return null;

	return upsertAccessUser(env, { sub, email, name: payload.name ?? email });
}

async function upsertAccessUser(
	env: Env,
	identity: { sub: string; email: string | null; name: string | null }
): Promise<SessionUser> {
	const db = getDb(env.ATTIC_DB);
	const id = `cfaccess:${identity.sub}`;
	const now = new Date();

	const existing = await db.select().from(schema.user).where(eq(schema.user.id, id)).limit(1);
	if (existing.length > 0) {
		const u = existing[0];
		return {
			id: u.id,
			sub: identity.sub,
			provider: 'cf-access',
			email: u.email,
			name: u.name,
			role: (u.role as SessionUser['role']) ?? 'member'
		};
	}

	await db.insert(schema.user).values({
		id,
		name: identity.name ?? identity.email ?? identity.sub,
		email: identity.email ?? `${identity.sub}@cf-access.local`,
		emailVerified: true,
		role: 'member',
		createdAt: now,
		updatedAt: now
	});

	return {
		id,
		sub: identity.sub,
		provider: 'cf-access',
		email: identity.email,
		name: identity.name,
		role: 'member'
	};
}
