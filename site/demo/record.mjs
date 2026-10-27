// Records the marketing clips against the mkt-demo stack.
//
//   source site/demo/env.sh && node site/demo/record.mjs hero live-session ...
//   node site/demo/record.mjs --no-seed <slug>   skip the reseed between clips
//   node site/demo/record.mjs --list
import { execFileSync } from 'node:child_process';
import { mkdirSync, renameSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { api, launch, UI, S, sleep } from './lib.mjs';
import { FakeDaemon, DIR, S6, TODOS, registerFixture, handleSpawns, heroStory, subagentStory, askStory } from './live-driver.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, '../..');
const OUT = resolve(here, 'out');
const CI_KEY = 'cctui_m_seed-fixture-ci-runner-02';

const CURSOR_CSS = `
#demo-cursor{position:fixed;left:0;top:0;width:22px;height:22px;z-index:2147483647;pointer-events:none;
 transform:translate(-4px,-3px);transition:left .45s cubic-bezier(.3,.7,.3,1),top .45s cubic-bezier(.3,.7,.3,1);opacity:0}
#demo-cursor svg{filter:drop-shadow(0 1px 2px rgba(0,0,0,.6))}
#demo-cursor.show{opacity:1}
#demo-cursor.press{transform:translate(-4px,-3px) scale(.85)}
#demo-ripple{position:fixed;width:36px;height:36px;border-radius:50%;z-index:2147483646;pointer-events:none;
 border:2px solid #4ade80;background:rgba(74,222,128,.25);transform:translate(-50%,-50%) scale(.3);opacity:0}
#demo-ripple.go{animation:demo-rip .5s ease-out}
@keyframes demo-rip{0%{opacity:.9;transform:translate(-50%,-50%) scale(.3)}100%{opacity:0;transform:translate(-50%,-50%) scale(1.4)}}
`;
const CURSOR_SVG = `<svg viewBox="0 0 24 24" width="22" height="22"><path d="M4 2l14 11-6.2.9L15.5 21l-3.2 1.3-3.7-7.1L4 19z" fill="#fff" stroke="#111" stroke-width="1.4" stroke-linejoin="round"/></svg>`;

function cursorInit({ css, svg }) {
	const mount = () => {
		if (document.getElementById('demo-cursor')) return;
		const style = document.createElement('style');
		style.textContent = css;
		document.head.appendChild(style);
		const c = document.createElement('div');
		c.id = 'demo-cursor';
		c.innerHTML = svg;
		const r = document.createElement('div');
		r.id = 'demo-ripple';
		document.body.append(c, r);
	};
	window.__demoCursor = {
		move(x, y) {
			mount();
			const c = document.getElementById('demo-cursor');
			c.classList.add('show');
			c.style.left = x + 'px';
			c.style.top = y + 'px';
		},
		press() {
			mount();
			const c = document.getElementById('demo-cursor');
			c.classList.add('press');
			const r = document.getElementById('demo-ripple');
			r.style.left = c.style.left;
			r.style.top = c.style.top;
			r.classList.remove('go');
			void r.offsetWidth;
			r.classList.add('go');
			setTimeout(() => c.classList.remove('press'), 160);
		},
		hide() {
			document.getElementById('demo-cursor')?.classList.remove('show');
		}
	};
	if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', mount);
	else mount();
}

class Demo {
	constructor(page, { mobile }) {
		this.page = page;
		this.mobile = mobile;
		this.x = 640;
		this.y = 400;
	}

	async goto(path, settle = 1500) {
		await this.page.goto(UI + path, { waitUntil: 'networkidle' });
		await sleep(settle);
	}

	async cursor(x, y) {
		this.x = x;
		this.y = y;
		await this.page.evaluate(([x, y]) => window.__demoCursor?.move(x, y), [x, y]);
	}

	async hover(sel, { dx = 0, dy = 0, settle = 550 } = {}) {
		const loc = typeof sel === 'string' ? this.page.locator(sel).first() : sel;
		await loc.waitFor({ state: 'visible', timeout: 15000 });
		await loc.scrollIntoViewIfNeeded();
		const box = await loc.boundingBox();
		const x = box.x + Math.min(box.width / 2, 60) + dx;
		const y = box.y + box.height / 2 + dy;
		await this.cursor(x, y);
		await this.page.mouse.move(x, y, { steps: 8 });
		await sleep(settle);
		return { x, y, loc };
	}

	async click(sel, opts = {}) {
		const { x, y } = await this.hover(sel, opts);
		await this.page.evaluate(() => window.__demoCursor?.press());
		await this.page.mouse.click(x, y);
		await sleep(opts.after ?? 700);
	}

	async type(text, { delay = 38 } = {}) {
		await this.page.keyboard.type(text, { delay });
	}

	async scroll(sel, dy, { steps = 10, pause = 60 } = {}) {
		const loc = this.page.locator(sel).first();
		const box = await loc.boundingBox();
		await this.page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
		for (let i = 0; i < steps; i++) {
			await this.page.mouse.wheel(0, dy / steps);
			await sleep(pause);
		}
	}

	async dismissColdCache() {
		const banner = this.page.locator('.cold-offer button[aria-label="Dismiss"]');
		await banner.waitFor({ state: 'visible', timeout: 1500 }).catch(() => {});
		if (await banner.isVisible().catch(() => false)) await this.click(banner, { after: 400 });
	}

	async selectTheme(id) {
		await this.page.evaluate((id) => document.documentElement.setAttribute('data-theme', id), id);
	}
}

const session = (id) => `[data-journey="session"]:has([data-journey="title"]):has-text("${id}")`;
const title = (text) => `[data-journey="session"] [data-journey="title"]:has-text("${text}")`;
const composer = '[data-journey="composer"] textarea';

async function selectMachine(d, name) {
	const value = await d.page.$eval('dialog[open] select', (el, name) => [...el.options].find((o) => o.textContent.includes(name))?.value, name);
	await d.page.selectOption('dialog[open] select', value);
}

async function openDrawer(d, text) {
	await d.click(title(text), { after: 1200 });
	await d.dismissColdCache();
}

const scenarios = {
	async hero({ d, daemon }) {
		await d.goto('/sessions', 1000);
		await d.cursor(900, 300);
		await sleep(800);
		const story = heroStory(daemon, { pace: 0.75 });
		await openDrawer(d, 'Add pagination');
		const allow = d.page.getByRole('button', { name: 'Allow', exact: true });
		await allow.waitFor({ state: 'visible', timeout: 30000 });
		await sleep(900);
		await d.click(allow, { settle: 800, after: 300 });
		await d.cursor(1230, 700);
		await story;
		await sleep(700);
		await d.click(d.page.getByRole('button', { name: 'Back', exact: true }), { after: 1000 });
		await d.hover(title('Edge cases'), { settle: 1000 });
		await d.click('[data-journey="new"]', { after: 700 });
		await selectMachine(d, 'ci-runner-02');
		await sleep(400);
		await d.click('dialog[open] input[placeholder="/home/user/project"]', { after: 150 });
		await d.page.fill('dialog[open] input[placeholder="/home/user/project"]', '');
		await d.type('/work/acme/platform-infra', { delay: 14 });
		await sleep(700);
		await d.click('dialog[open] [data-journey="label"]', { after: 150 });
		await d.page.fill('dialog[open] [data-journey="label"]', 'Pin the base images');
		await sleep(400);
		await d.click('dialog[open] [data-journey="prompt"]', { after: 150 });
		await d.type('Pin every base image in deploy/ to a digest.', { delay: 14 });
		await d.click(d.page.getByRole('button', { name: /^Launch/ }), { after: 1800 });
		await d.hover(title('Pin the base images'), { settle: 1200 });
		await sleep(1200);
	},

	async 'live-session'({ d, daemon }) {
		await d.goto('/sessions');
		await openDrawer(d, 'Add pagination');
		const s1 = S.s1;
		daemon.status(s1, { tempo: 'active', state: 'working', activity: 'Writing tests/orders_pagination.rs' });
		daemon.thinking(s1, 'The two-page walk needs twelve orders so the second page is non-empty and the cursor from page one lands exactly on order seven.');
		await sleep(2200);
		const w = daemon.tool(s1, 'Write', { file_path: `${DIR}/tests/orders_pagination.rs`, content: '#[tokio::test]\nasync fn walks_two_pages() {\n    let app = test_app().await;\n    seed_orders(&app, 12).await;\n    let first: Page = get(&app, "/orders?limit=7").await;\n    assert_eq!(first.orders.len(), 7);\n}\n' });
		await sleep(1500);
		daemon.result(s1, w, 'Wrote 8 lines to tests/orders_pagination.rs');
		await sleep(1000);
		const t = daemon.tool(s1, 'Bash', { command: 'cargo test -p checkout-api --test orders_pagination', description: 'Run the new test' });
		daemon.status(s1, { activity: 'Running cargo test' });
		await sleep(1800);
		daemon.result(s1, t, 'running 1 test\ntest walks_two_pages ... ok\n\ntest result: ok. 1 passed; 0 failed; finished in 0.84s');
		await sleep(900);
		daemon.todos(s1, TODOS(3, 3));
		daemon.say(s1, 'The two-page walk passes. Moving on to the API reference.');
		await sleep(1600);
		await d.click(composer, { after: 300 });
		await d.type('Also cap limit at 200 and document it.', { delay: 34 });
		await sleep(400);
		await d.page.keyboard.press('Enter');
		await sleep(1600);
		daemon.thinking(s1, 'A cap belongs in the Cursor extractor so every caller gets it.');
		await sleep(1400);
		const e = daemon.tool(s1, 'Edit', { file_path: `${DIR}/src/routes/orders.rs`, old_string: 'let limit = page.limit.unwrap_or(50);', new_string: 'let limit = page.limit.unwrap_or(50).min(200);' });
		await sleep(1200);
		daemon.result(s1, e, 'Applied 1 edit to src/routes/orders.rs');
		await sleep(800);
		daemon.say(s1, 'Capped at 200 and noted in docs/api/orders.md.');
		await sleep(2200);
	},

	async permissions({ d, daemon }) {
		await d.goto('/sessions');
		await openDrawer(d, 'Fix the flaky checkout test');
		await sleep(800);
		const perm = daemon.permission(S.s2, 'Bash', { command: 'cargo test checkout -- --test-threads=1', description: 'Run the checkout tests serially' });
		await sleep(1800);
		await d.click(d.page.getByRole('button', { name: 'Allow', exact: true }), { settle: 900, after: 900 });
		await perm;
		const t = daemon.tool(S.s2, 'Bash', { command: 'cargo test checkout -- --test-threads=1', description: 'Run the checkout tests serially' });
		await sleep(1500);
		daemon.result(S.s2, t, 'test result: ok. 20 passed; 0 failed (serial run)');
		await sleep(600);
		const ask = askStory(daemon);
		await sleep(2200);
		await d.click(d.page.getByText('Isolate the fixture', { exact: true }), { settle: 900, after: 700 });
		await d.click(d.page.getByRole('button', { name: 'Send answer', exact: true }), { after: 1200 });
		await ask;
		await sleep(3000);
	},

	async spawn({ d, ci }) {
		await d.goto('/sessions');
		await d.click('[data-journey="new"]', { after: 1000 });
		await d.hover('dialog[open] select', { settle: 600 });
		await selectMachine(d, 'ci-runner-02');
		await sleep(600);
		await d.click('dialog[open] input[placeholder="/home/user/project"]', { after: 200 });
		await d.page.fill('dialog[open] input[placeholder="/home/user/project"]', '');
		await d.type('/work/acme/platform-infra', { delay: 24 });
		await sleep(900);
		await d.click('dialog[open] [data-journey="label"]', { after: 200 });
		await d.page.fill('dialog[open] [data-journey="label"]', 'Pin the base images');
		await sleep(500);
		await d.click('dialog[open] [data-journey="prompt"]', { after: 200 });
		await d.type('Pin every base image in deploy/ to a digest and open a pull request.', { delay: 20 });
		await d.hover('dialog[open] [data-journey="profile"]', { settle: 900 });
		await d.click(d.page.getByRole('button', { name: /^Launch/ }), { after: 2500 });
		await d.hover(title('Pin the base images'), { settle: 2500 });
		await openDrawer(d, 'Pin the base images');
		await sleep(3000);
	},

	async subagents({ d, daemon }) {
		await d.goto('/sessions');
		await d.hover(session('Audit the orders'), { settle: 1200 });
		const story = heroStory(daemon, { pace: 0.5, skipPermission: true });
		await story;
		await sleep(1200);
		await d.hover(title('Edge cases'), { settle: 1200 });
		await openDrawer(d, 'Edge cases');
		await subagentStory(daemon, { pace: 1 });
		await sleep(2000);
		await d.click(d.page.getByRole('button', { name: 'Back', exact: true }), { after: 2500 });
	},

	async usage({ d }) {
		await d.goto('/');
		await d.cursor(1000, 300);
		await sleep(1500);
		await d.scroll('main', 900, { steps: 18, pause: 70 });
		await sleep(1800);
		await d.click('a[href="/accounts"]', { after: 2000 });
		await d.hover('text=acme-team', { settle: 1500 });
		await d.scroll('main', 500, { steps: 12, pause: 70 });
		await sleep(2000);
	},

	async search({ d }) {
		await d.goto('/sessions');
		const box = '[data-journey="search"] input';
		await d.click(box, { after: 300 });
		await d.type('pagination', { delay: 60 });
		await sleep(2200);
		await d.page.keyboard.press('Control+A');
		await d.type('label:backend', { delay: 55 });
		await sleep(2200);
		await d.page.keyboard.press('Control+A');
		await d.type('machine:ci-runner-02', { delay: 50 });
		await sleep(2200);
		await d.page.keyboard.press('Control+A');
		await d.type('status:blocked', { delay: 50 });
		await sleep(2500);
	},

	async themes({ d }) {
		await d.goto('/sessions');
		await d.click('button[aria-label^="Theme"]', { after: 900 });
		for (const t of ['tokyonight', 'gruvbox', 'dracula', 'rosepine', 'nord', 'light', 'latte', 'solarizedlight', 'sepia', 'dark']) {
			await d.click(`.cell:has(.swatch[data-theme="${t}"])`, { settle: 350, after: 950 });
		}
		await d.page.keyboard.press('Escape');
		await sleep(1200);
	},

	async mobile({ d, daemon }) {
		await d.goto('/sessions');
		await sleep(800);
		const ask = askStory(daemon);
		await sleep(600);
		await d.click(title('Fix the flaky'), { after: 1500 });
		await d.click(d.page.getByText('Isolate the fixture', { exact: true }), { settle: 900, after: 700 });
		await d.click(d.page.getByRole('button', { name: 'Send answer', exact: true }), { after: 1500 });
		await ask;
		await sleep(2200);
		await d.click(d.page.getByRole('button', { name: 'Back', exact: true }), { after: 2500 });
	},

	async harnesses({ d }) {
		await d.goto('/sessions', 1000);
		await d.cursor(700, 300);
		await d.scroll('main', 400, { steps: 8, pause: 50 });
		await sleep(600);
		for (const name of ['Migrate the cart store', 'Draft the 2.4 release notes', 'Update the onboarding copy']) {
			await openDrawer(d, name);
			await sleep(1300);
			await d.click(d.page.getByRole('button', { name: 'Back', exact: true }), { after: 400 });
		}
		await sleep(800);
	},

	async rooms({ d, daemon }) {
		await d.goto('/sessions');
		await d.hover('text=checkout-pagination', { settle: 1200 });
		await openDrawer(d, 'Add pagination');
		await sleep(1500);
		daemon.msg(S.s1, 'user', '<cctui-room name="checkout-pagination" from="Audit the orders endpoint tests (claude-code on workstation-01)">\nStale cursor now returns 400 with {"error":"stale_cursor"}. Both edge-case tests pass.\n</cctui-room>');
		await sleep(2000);
		daemon.say(S.s1, 'Thanks. Documenting the 400 in the API reference now.');
		await sleep(2000);
		await d.hover('[data-journey="conversation"] >> text=checkout-pagination', { settle: 2000 });
		await sleep(1000);
	},

	async workers({ d }) {
		await d.goto('/settings/dispatchers');
		await d.hover('text=kube-eu', { settle: 1800 });
		await d.goto('/dispatch-failures');
		await d.hover('text=OOMKilled', { settle: 1500 });
		await d.hover('text=Last worker log lines', { settle: 1800 });
		await d.scroll('main', 300, { steps: 8, pause: 70 });
		await sleep(2500);
	},

	async plugins({ d }) {
		await d.goto('/settings/plugins');
		await d.hover('text=Page demo', { settle: 1800 });
		await d.scroll('main', 400, { steps: 8, pause: 70 });
		await sleep(1200);
		await d.goto('/apps/pagedemo');
		await d.cursor(700, 400);
		await sleep(2500);
		const link = d.page.locator('main a').first();
		if (await link.isVisible().catch(() => false)) await d.click(link, { after: 2000 });
	},

	async fleet({ d }) {
		await d.goto('/settings/users');
		await d.click('[data-journey="user"]', { after: 1200 });
		await d.click(d.page.getByText(/^Machines \d/), { after: 1500 });
		await d.hover(d.page.getByText('cctui_m_…tion-01'), { settle: 1200 });
		await d.hover(d.page.getByText('cctui_m_…box-03'), { settle: 1500 });
		await d.goto('/settings/users', 800);
		await d.hover('[data-journey="enroll-cmd"]', { settle: 1500 });
		await d.click('[data-journey="enroll-copy"]', { after: 2000 });
	},

	async voice({ d }) {
		await d.goto('/sessions');
		await openDrawer(d, 'Add pagination');
		const voice = d.page.locator('text=Keep the response shape exactly').first();
		await voice.scrollIntoViewIfNeeded();
		await sleep(800);
		await d.hover(voice, { settle: 1500 });
		const play = d.page.locator('[data-journey="conversation"] button[aria-label*="Play"], [data-journey="conversation"] button:has-text("Play")').first();
		if (await play.isVisible().catch(() => false)) await d.click(play, { after: 2500 });
		await d.click(d.page.locator('summary:has-text("Transcript")').first(), { after: 2500 }).catch(() => {});
		await sleep(1000);
	}
};

const MOBILE = new Set(['mobile']);
const NEEDS_CI = new Set(['hero', 'spawn']);

function psql(sql) {
	execFileSync('docker', ['compose', '-p', 'mkt-demo', '-f', `${root}/deploy/local/docker-compose.yaml`, '--env-file', `${root}/tmp/mkt/.env`, 'exec', '-T', 'postgres', 'psql', '-U', 'postgres', '-d', 'cctui', '-q', '-c', sql], { stdio: 'inherit' });
}

async function clearPendingPrompts(daemon) {
	const pending = await api('GET', '/permissions/pending').catch(() => []);
	for (const p of Array.isArray(pending) ? pending : pending?.pending ?? []) {
		daemon.event('permission_resolved', { local_id: p.session_id, request_id: p.request_id });
	}
	for (const id of [S.s1, S.s2, S.s5]) {
		daemon.event('ask_resolved', { local_id: id });
		daemon.event('plan_resolved', { local_id: id });
	}
}

async function record(slug, { seed }) {
	if (!scenarios[slug]) throw new Error(`unknown clip ${slug}`);
	if (seed) execFileSync('bash', [`${here}/seed.sh`], { stdio: 'inherit' });
	if (slug !== 'workers') psql("UPDATE dispatch_records SET created_at = created_at - interval '8 days' WHERE dispatcher_name = 'kube-eu'");
	const daemon = await new FakeDaemon().connect();
	await clearPendingPrompts(daemon);
	registerFixture(daemon);
	handleSpawns(daemon);
	let ci = null;
	if (NEEDS_CI.has(slug)) {
		ci = await new FakeDaemon({ key: CI_KEY }).connect();
		handleSpawns(ci);
	}
	await sleep(1200);
	const mobile = MOBILE.has(slug);
	const videoDir = resolve(OUT, `video-${slug}`);
	mkdirSync(videoDir, { recursive: true });
	const { browser, ctx, page } = await launch({ mobile, video: videoDir });
	await page.addInitScript(cursorInit, { css: CURSOR_CSS, svg: CURSOR_SVG });
	const d = new Demo(page, { mobile });
	const started = Date.now();
	try {
		await scenarios[slug]({ d, daemon, ci });
	} finally {
		const video = page.video();
		await ctx.close();
		await browser.close();
		daemon.close();
		ci?.close();
		const src = await video.path();
		renameSync(src, resolve(OUT, `${slug}.webm`));
		console.log(`${slug}: ${((Date.now() - started) / 1000).toFixed(1)}s -> site/demo/out/${slug}.webm`);
	}
}

const args = process.argv.slice(2);
if (args.includes('--list')) {
	console.log(Object.keys(scenarios).join('\n'));
	process.exit(0);
}
const seed = !args.includes('--no-seed');
const slugs = args.filter((a) => !a.startsWith('--'));
for (const slug of slugs) await record(slug, { seed });
