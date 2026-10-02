/** Top-level screens that became Settings pages. The old paths stay as
 *  permanent redirects so existing deep links and guides keep resolving. */
export const MOVED_ROUTES = {
	'/access': '/settings/users',
	'/accounts': '/settings/accounts',
	'/dispatchers': '/settings/dispatchers'
} as const;
