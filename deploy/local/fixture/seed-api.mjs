// The API half of the demo fixture: an AI account and the spawn memory that
// makes the new-session dialog open on a known folder. The DB half (machines,
// sessions, transcripts) is seed.sql, and seed.sh runs both in the right order.
//
//   CCTUI_TOKEN=dev-admin node deploy/local/fixture/seed-api.mjs [theme]

const vars = JSON.parse(process.env.JOURNEY_VARS ?? '{}');
const token = vars.token ?? process.env.CCTUI_TOKEN;
const api = vars.api ?? process.env.CCTUI_API_URL ?? 'http://localhost:8700';
const USER = '00000000-0000-0000-0000-000000000000';
const MACHINE = 'c0000000-0000-4000-8000-000000000001';
const DIR = '/work/acme/checkout-api';

if (!token) {
	console.error('fixture: no token — set JOURNEY_VARS=\'{"token":"…"}\'');
	process.exit(1);
}

const call = async (method, path, body) => {
	const res = await fetch(`${api}/api/v1${path}`, {
		method,
		headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' },
		body: body === undefined ? undefined : JSON.stringify(body)
	});
	if (!res.ok) throw new Error(`${method} ${path} → ${res.status} ${await res.text()}`);
	return res.status === 204 ? null : res.json();
};

const accounts = await call('GET', '/accounts');
if (!accounts.some((a) => a.name === 'acme-team')) {
	await call('POST', '/accounts', {
		name: 'acme-team',
		emoji: '🛠',
		user_id: USER,
		provider: 'anthropic',
		refresh_token: 'fixture-not-a-real-token',
		access_token: 'fixture-not-a-real-token',
		expires_at: 4102444800
	});
	console.log('fixture: created account acme-team');
}

// The theme the app resolves on load. The book renders one variant per pass
// because the store re-applies this over whatever the driver set.
const themeArg = process.argv[2];

// `m<US><machine><US><dir>` — the key spawnMemory.ts builds. Seeding it is what
// makes the dialog open on the last folder used, as it does for a returning user.
const key = `m${MACHINE}${DIR}`;
const settings = await call('GET', '/settings');
const data = settings.data ?? {};
data.spawnMemory = {
	...(data.spawnMemory ?? {}),
	[key]: {
		adapter_id: 'claude-code',
		model_claude: 'opus',
		model_codex: '',
		model_account: '',
		effort_claude: 'medium',
		effort_codex: '',
		account: '',
		account_provider: '',
		permission_mode: 'default',
		name: '',
		at: Date.now()
	}
};
// `live` is the server's in-memory registry, not a DB column: a session only
// counts as live once it has registered, so the fixture registers its three
// running sessions here. The DB seed runs again afterwards to restore the
// fields registration resets.
for (const n of ['1', '2', '5']) {
	await call('POST', '/sessions/register', {
		claude_session_id: `a0000000-0000-4000-8000-00000000000${n}`,
		machine_id: 'workstation-01',
		working_dir: '/work/acme/checkout-api'
	});
}
console.log('fixture: registered 3 running sessions');

data.secretScrubEnabled = true;

if (themeArg) {
	data.display = { ...(data.display ?? {}), theme: themeArg };
}
await call('PUT', '/settings', { version: settings.version ?? 1, data });
console.log('fixture: spawn memory points at', DIR, themeArg ? `· theme ${themeArg}` : '');
