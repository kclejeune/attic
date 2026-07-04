// See https://svelte.dev/docs/kit/types#app.d.ts
// for information about these interfaces

import type { D1Database, Fetcher } from '@cloudflare/workers-types';
import type { SessionUser } from '$lib/server/auth/types';

declare global {
	namespace App {
		// interface Error {}

		interface Locals {
			/** The authenticated user, or null for anonymous requests. */
			user: SessionUser | null;
		}

		// interface PageData {}
		// interface PageState {}

		interface Platform {
			env: {
				ATTIC_DB: D1Database;
				ATTIC_API: Fetcher;
				APP_URL?: string;
				JWT_HS256_SECRET_BASE64?: string;
				SESSION_SECRET?: string;
				OIDC_ISSUER?: string;
				OIDC_CLIENT_ID?: string;
				OIDC_CLIENT_SECRET?: string;
				OIDC_REDIRECT_URI?: string;
				CF_ACCESS_TEAM_DOMAIN?: string;
				CF_ACCESS_AUD?: string;
			};
			ctx: ExecutionContext;
		}
	}
}

export {};
