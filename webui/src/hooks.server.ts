import type { Handle } from '@sveltejs/kit';
import { FX_PREPAINT, THEME_PREPAINT } from '@dorsk/tsumikit';

// Runs at prerender, so the static shell paints the saved theme, look and
// effects before the app boots. A bare <script> so nginx stamps its CSP nonce.
const PREPAINT = `<script>${THEME_PREPAINT}${FX_PREPAINT}</script>`;

export const handle: Handle = ({ event, resolve }) =>
	resolve(event, {
		transformPageChunk: ({ html }) => html.replace('%cctui.prepaint%', PREPAINT)
	});
