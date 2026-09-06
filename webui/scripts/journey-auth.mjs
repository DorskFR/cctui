// Mints the browser storage state journeys replay with. The token arrives in
// JOURNEY_VARS (or CCTUI_TOKEN) and only ever reaches the login endpoint; the
// state file it writes is gitignored.
import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const out = resolve(here, '../journeys/.auth/state.json');

const vars = JSON.parse(process.env.JOURNEY_VARS ?? '{}');
const token = vars.token ?? process.env.CCTUI_TOKEN;
const app = vars.app ?? process.env.JOURNEY_APP_URL ?? 'http://localhost:8088';

if (!token) {
	console.error('journey-auth: no token — set JOURNEY_VARS=\'{"token":"…"}\'');
	process.exit(1);
}

const res = await fetch(`${app}/api/v1/auth/login`, {
	method: 'POST',
	headers: { 'Content-Type': 'application/json' },
	body: JSON.stringify({ token })
});
if (!res.ok) {
	console.error(`journey-auth: login failed with ${res.status}`);
	process.exit(1);
}

const raw = res.headers.getSetCookie?.() ?? [];
const cookies = raw
	.map((line) => line.split(';')[0].split('='))
	.filter(([name]) => name)
	.map(([name, ...rest]) => ({
		name,
		value: rest.join('='),
		domain: new URL(app).hostname,
		path: '/',
		expires: Math.floor(Date.now() / 1000) + 31536000,
		httpOnly: true,
		secure: new URL(app).protocol === 'https:',
		sameSite: 'Lax'
	}));

if (cookies.length === 0) {
	console.error('journey-auth: login returned no cookie');
	process.exit(1);
}

mkdirSync(dirname(out), { recursive: true });
writeFileSync(out, JSON.stringify({ cookies, origins: [] }, null, 2));
console.log(`journey-auth: wrote ${cookies.length} cookie(s) to journeys/.auth/state.json`);
