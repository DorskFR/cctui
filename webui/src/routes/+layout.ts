// SPA / app mode: render everything client-side. The fallback index.html is
// prerendered; all data loads happen in the browser against the API.
export const ssr = false;
export const prerender = true;
export const trailingSlash = 'ignore';
