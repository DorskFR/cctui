/** Top-level screens that became Settings pages. The old paths stay as
 *  permanent redirects so existing deep links and guides keep resolving. */
export const MOVED_ROUTES = {
	'/access': '/settings/users',
	'/dispatchers': '/settings/dispatchers',
	'/events': '/settings/events'
} as const;
