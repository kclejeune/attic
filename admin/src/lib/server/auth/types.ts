/** How a user authenticated. */
export type AuthProvider = 'oidc' | 'cf-access';

/** Application role, controlling access to admin functions. */
export type UserRole = 'admin' | 'member';

/** A normalized identity produced by any auth provider. */
export interface Identity {
	/** Stable subject identifier from the provider. */
	sub: string;
	provider: AuthProvider;
	email: string | null;
	name: string | null;
}

/** The authenticated user attached to a request (`event.locals.user`). */
export interface SessionUser {
	id: number;
	sub: string;
	provider: AuthProvider;
	email: string | null;
	name: string | null;
	role: UserRole;
}
