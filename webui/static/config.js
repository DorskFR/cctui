// Runtime configuration — overridable per-deployment without rebuilding the
// SPA (mount/replace this file in the static deploy). `apiBase` is the cctui
// server origin (no trailing slash); the SPA appends `/api/v1/...`.
// Leave empty to target the same origin the SPA is served from.
window.CCTUI_CONFIG = {
	apiBase: ''
};
