// Asserts the conversation drawer keeps every interactive control inside the
// viewport at the widths the book is shot at (CCT-1015). Exits non-zero on the
// first control whose box crosses an edge.
//
//   CCTUI_TOKEN=dev-admin node scripts/check-drawer-clipping.mjs [--out DIR] [--tag before]
//
// Needs a served build (vite preview) in APP_URL proxying a seeded API.
import { mkdirSync } from 'node:fs';
import { chromium } from 'playwright';

const app = process.env.JOURNEY_APP_URL ?? 'http://localhost:5291';
const token = process.env.CCTUI_TOKEN ?? 'dev-admin';
const args = process.argv.slice(2);
const arg = (name, fallback) => {
	const i = args.indexOf(`--${name}`);
	return i >= 0 ? args[i + 1] : fallback;
};
const outDir = arg('out', '');
const tag = arg('tag', 'after');
const WIDTHS = [1024, 1280, 1440];
// The drawer is user-resizable, so the default width alone does not cover it:
// each viewport is also measured at a persisted width the user can drag to.
const DRAWER_WIDTHS = [null, 700, 800];
// Sub-pixel rounding of a box that ends exactly on the edge is not clipping.
const EPSILON = 0.5;

const res = await fetch(`${app}/api/v1/auth/login`, {
	method: 'POST',
	headers: { 'Content-Type': 'application/json' },
	body: JSON.stringify({ token })
});
if (!res.ok) throw new Error(`login failed: ${res.status}`);
const cookies = (res.headers.getSetCookie?.() ?? []).map((line) => {
	const [name, ...rest] = line.split(';')[0].split('=');
	return {
		name,
		value: rest.join('='),
		domain: new URL(app).hostname,
		path: '/',
		expires: Math.floor(Date.now() / 1000) + 3600,
		httpOnly: true,
		secure: false,
		sameSite: 'Lax'
	};
});
if (cookies.length === 0) throw new Error('login returned no cookie');

if (outDir) mkdirSync(outDir, { recursive: true });

const browser = await chromium.launch();
const failures = [];
const rows = [];

for (const width of WIDTHS) {
	for (const drawerWidth of DRAWER_WIDTHS) {
	const ctx = await browser.newContext({
		viewport: { width, height: 800 },
		colorScheme: 'dark'
	});
	await ctx.addCookies(cookies);
	const page = await ctx.newPage();
	await page.goto(`${app}/sessions`);
	await page.evaluate((w) => {
		if (w === null) localStorage.removeItem('cctui_drawer_width');
		else localStorage.setItem('cctui_drawer_width', String(w));
	}, drawerWidth);
	await page.goto(`${app}/sessions`);

	await page.locator('[data-journey="section"] a, [data-journey="section"] [role="button"]').first().waitFor();
	await page.locator('[data-journey="section"]').first().getByText(/pagination/i).first().click();
	await page.locator('[data-journey="conversation"]').waitFor();
	await page.locator('[data-journey="composer"]').waitFor();
	await page.waitForTimeout(800);

	const measured = await page.evaluate(() => {
		const drawer = document.querySelector('[data-journey="conversation"]');
		const composer = document.querySelector('[data-journey="composer"]');
		const head = drawer?.querySelector('.dhead') ?? null;
		const sel = 'button, a[href], input, select, textarea, [role="button"], [tabindex]:not([tabindex="-1"])';
		const pick = (root, zone) =>
			root
				? [...root.querySelectorAll(sel)]
						.filter((el) => {
							const r = el.getBoundingClientRect();
							const cs = getComputedStyle(el);
							return (
								r.width > 0 &&
								r.height > 0 &&
								cs.visibility !== 'hidden' &&
								cs.display !== 'none'
							);
						})
						.map((el) => {
							const r = el.getBoundingClientRect();
							return {
								zone,
								label:
									el.getAttribute('aria-label') ||
									el.getAttribute('title') ||
									(el.textContent ?? '').trim().slice(0, 40) ||
									el.tagName.toLowerCase(),
								left: r.left,
								right: r.right,
								top: r.top,
								bottom: r.bottom
							};
						})
				: [];
		const boxOf = (el) => {
			if (!el) return null;
			const r = el.getBoundingClientRect();
			return { left: r.left, right: r.right, scrollWidth: el.scrollWidth, clientWidth: el.clientWidth };
		};
		return {
			controls: [...pick(head, 'header'), ...pick(composer, 'footer')],
			drawer: boxOf(drawer),
			head: boxOf(head)
		};
	});

	const label = drawerWidth === null ? 'default' : `${drawerWidth}`;
	rows.push({ width, dw: label, ...measured });

	for (const c of measured.controls) {
		if (c.left < -EPSILON || c.right > width + EPSILON) {
			failures.push(`${width}px (drawer ${label}) ${c.zone} "${c.label}": left=${c.left.toFixed(1)} right=${c.right.toFixed(1)} (viewport 0..${width})`);
		}
	}

	if (outDir) {
		const suffix = drawerWidth === null ? '' : `-drawer${drawerWidth}`;
		await page.screenshot({ path: `${outDir}/${tag}-${width}${suffix}.png` });
	}
	await ctx.close();
	}
}

await browser.close();

for (const r of rows) {
	console.log(
		`\n── viewport ${r.width}px · drawer width ${r.dw} — box ${r.drawer?.left.toFixed(1)}..${r.drawer?.right.toFixed(1)} ` +
			`(scrollWidth ${r.drawer?.scrollWidth} / clientWidth ${r.drawer?.clientWidth}) · ` +
			`header scrollWidth ${r.head?.scrollWidth} / clientWidth ${r.head?.clientWidth}`
	);
	for (const c of r.controls) {
		const bad = c.left < -EPSILON || c.right > r.width + EPSILON;
		console.log(`  ${bad ? 'CLIP' : '  ok'}  ${c.zone.padEnd(6)} ${c.left.toFixed(1).padStart(7)}..${c.right.toFixed(1).padStart(7)}  ${c.label}`);
	}
}

if (failures.length > 0) {
	console.error(`\n${failures.length} clipped control(s):`);
	for (const f of failures) console.error(`  ${f}`);
	process.exit(1);
}
console.log(`\nOK — no clipped controls at ${WIDTHS.join(', ')}px`);
