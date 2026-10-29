// API half of the extra fixture: macros and an account redirect.
import { api } from './lib.mjs';

const settings = await api('GET', '/settings');
const data = settings.data ?? {};
data.macros = {
	enabled: true,
	items: [
		{ id: 'm-review', title: 'Review this branch', prompt: 'Review the open pull request on this branch. Flag anything that changes a public API.', adapter: 'claude-code', model: 'sonnet', effort: 'low', permission_mode: 'ask', confirm: false },
		{ id: 'm-flaky', title: 'Hunt a flaky test', prompt: 'Run the test suite ten times and explain any test that does not pass every run.', adapter: 'claude-code', model: 'opus', effort: 'medium', permission_mode: 'auto', confirm: true }
	]
};
await api('PUT', '/settings', { version: settings.version ?? 1, data });
console.log('extra: macros stored');

const accounts = await api('GET', '/accounts');
const byName = new Map(accounts.map((a) => [a.name, a.id]));
await api('PUT', `/accounts/${byName.get('acme-team')}/redirect`, {
	to_account: byName.get('acme-research'),
	family: 'anthropic',
	until: new Date(Date.now() + 2 * 3600 * 1000).toISOString(),
	reason: 'demo: acme-team is near its weekly window',
	user_id: '00000000-0000-0000-0000-000000000000'
}).then(() => console.log('extra: redirect acme-team -> acme-research')).catch((e) => console.log('extra: redirect skipped:', e.message.slice(0, 160)));
