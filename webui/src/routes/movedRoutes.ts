/** Top-level screens that became Settings subsections. The old paths stay as
 *  permanent redirects so existing deep links and guides keep resolving. */
export const MOVED_ROUTES = {
	'/access': '/settings/instance#users',
	'/dispatchers': '/settings/instance#dispatchers'
} as const;
