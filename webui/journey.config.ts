import { defineConfig } from '@dorsk/journey';
import { isLive } from './src/lib/journeys/live';

const url = process.env.JOURNEY_APP_URL ?? 'http://localhost:5273';
const api = process.env.CCTUI_PROXY ?? 'http://localhost:8700';

export default defineConfig({
	// Replayed against the production build, not the dev server: a dev server
	// transforms modules on demand, so a cold first paint can swallow the first
	// click. An already-listening server is reused as-is.
	app: {
		url,
		start: 'npx vite preview --port 5273 --strictPort',
		env: { CCTUI_PROXY: api },
		timeout: 120000
	},
	journeys: 'journeys/*.journey.ts',
	// A step addressing a real row (`user[{fixture.me}]`) needs the name the
	// instance answers to. The app fills the same keys from `guideParams`.
	fixtures: {
		instance: {
			setup: async ({ baseUrl, request }) => {
				const get = async (path: string) => (await request.get(new URL(path, baseUrl).href)).json();
				const me = await get('/api/v1/me');
				const { sessions } = await get('/api/v1/sessions');
				const live = sessions.find(isLive);
				return live ? { me: me.user_name, session: live.id } : { me: me.user_name };
			}
		}
	},
	// The guided fills take any text; the book types these so the spawn-session
	// screenshots keep their captions.
	vars: {
		label: 'Add pagination to the orders endpoint',
		prompt: 'Add cursor pagination to GET /orders. Keep the response shape and cover it with a test.'
	},
	out: '../docs/journeys',
	storageState: 'journeys/.auth/state.json',
	// The captions ship as markdown beside each image; drawn onto the frame they
	// would cover the very UI the screenshot exists to record.
	presenter: 'none',
	variants: {
		viewport: {
			desktop: { width: 1280, height: 800 },
			mobile: { width: 390, height: 844 }
		},
		// A journey that names no theme gets the first entry, so the record is
		// dark unless it opts in. Themes come from the seeded settings blob, not
		// from the browser, so each one needs its own book pass.
		theme: ['dark', 'light', 'gruvbox']
	},
	pages: [
		'/',
		'/sessions',
		'/access',
		'/accounts',
		'/users',
		'/dispatchers',
		'/settings/appearance',
		'/settings/sessions',
		'/settings/execution',
		'/settings/privacy',
		'/settings/notifications',
		'/settings/security',
		'/settings/instance'
	]
});
