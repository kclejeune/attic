import type { Handle } from '@sveltejs/kit';

// Auth resolution is layered in by src/lib/server/auth. For now every request is
// anonymous; route guards live in the (app) group's server load.
export const handle: Handle = async ({ event, resolve }) => {
	event.locals.user = null;
	return resolve(event);
};
