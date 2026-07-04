// Mints attic-compatible JWTs (HS256) so the admin app can call the attic API
// over the service binding on behalf of a user. Mirrors the claim shape the
// attic token crate validates (namespace `https://jwt.attic.rs/v1`).

const CLAIM_NAMESPACE = 'https://jwt.attic.rs/v1';

/** Per-cache permission flags, using the attic short keys. */
export interface CachePermission {
	/** pull */ r?: 1;
	/** push */ w?: 1;
	/** delete */ d?: 1;
	/** create cache */ cc?: 1;
	/** configure cache */ cr?: 1;
	/** configure cache retention */ cq?: 1;
	/** destroy cache */ cd?: 1;
}

/** Map of cache name (or wildcard pattern) to permissions. */
export type CacheAccess = Record<string, CachePermission>;

function base64url(bytes: Uint8Array): string {
	let bin = '';
	for (const b of bytes) bin += String.fromCharCode(b);
	return btoa(bin).replace(/=+$/, '').replace(/\+/g, '-').replace(/\//g, '_');
}

function base64urlJson(value: unknown): string {
	return base64url(new TextEncoder().encode(JSON.stringify(value)));
}

/**
 * Mint a short-lived attic JWT.
 *
 * @param secretBase64 base64-encoded HS256 secret (same one the attic worker validates with)
 * @param sub subject (the acting user's id)
 * @param caches the cache access map to grant
 * @param ttlSeconds token lifetime (default 5 minutes — just long enough for a request)
 */
export async function mintAtticToken(
	secretBase64: string,
	sub: string,
	caches: CacheAccess,
	ttlSeconds = 300
): Promise<string> {
	const now = Math.floor(Date.now() / 1000);
	const header = { alg: 'HS256', typ: 'JWT' };
	const payload = {
		sub,
		iat: now,
		exp: now + ttlSeconds,
		[CLAIM_NAMESPACE]: { caches }
	};

	const signingInput = `${base64urlJson(header)}.${base64urlJson(payload)}`;

	const secret = Uint8Array.from(atob(secretBase64), (c) => c.charCodeAt(0));
	const key = await crypto.subtle.importKey(
		'raw',
		secret,
		{ name: 'HMAC', hash: 'SHA-256' },
		false,
		['sign']
	);
	const sig = await crypto.subtle.sign('HMAC', key, new TextEncoder().encode(signingInput));

	return `${signingInput}.${base64url(new Uint8Array(sig))}`;
}
