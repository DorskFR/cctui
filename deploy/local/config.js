// Runtime config for the local stack — mounted over the SPA's config.js so the
// browser talks to the cctui-server published on the host. If you change
// CCTUI_SERVER_PORT in docker-compose.yaml, update this origin to match.
window.CCTUI_CONFIG = {
	apiBase: 'http://localhost:8700'
};
