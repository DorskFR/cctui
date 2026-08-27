// Empty ⇒ same-origin /api (proxied to cctui-server by the local nginx.conf).
// A cross-origin apiBase here is blocked by the SPA's CSP (connect-src 'self').
window.CCTUI_CONFIG = {};
