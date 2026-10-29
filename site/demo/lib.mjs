import { chromium } from '../../webui/node_modules/playwright/index.mjs';

export const UI = process.env.DEMO_UI_URL ?? 'http://localhost:8788';
export const API = process.env.CCTUI_API_URL ?? 'http://localhost:8790';
export const TOKEN = process.env.CCTUI_TOKEN;
export const MACHINE_KEY = 'cctui_m_seed-fixture-workstation-01';

export const S = {
	s1: 'a0000000-0000-4000-8000-000000000001',
	s2: 'a0000000-0000-4000-8000-000000000002',
	s3: 'a0000000-0000-4000-8000-000000000003',
	s4: 'a0000000-0000-4000-8000-000000000004',
	s5: 'a0000000-0000-4000-8000-000000000005'
};

export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

export async function api(method, path, body, bearer = TOKEN) {
	const res = await fetch(`${API}/api/v1${path}`, {
		method,
		headers: { Authorization: `Bearer ${bearer}`, 'Content-Type': 'application/json' },
		body: body === undefined ? undefined : JSON.stringify(body)
	});
	const text = await res.text();
	if (!res.ok) throw new Error(`${method} ${path} -> ${res.status} ${text}`);
	return text ? JSON.parse(text) : null;
}

export async function loginCookies() {
	const res = await fetch(`${UI}/api/v1/auth/login`, {
		method: 'POST',
		headers: { 'Content-Type': 'application/json' },
		body: JSON.stringify({ token: TOKEN })
	});
	if (!res.ok) throw new Error(`login ${res.status}`);
	const host = new URL(UI).hostname;
	return (res.headers.getSetCookie?.() ?? []).map((line) => {
		const [name, ...rest] = line.split(';')[0].split('=');
		return { name, value: rest.join('='), domain: host, path: '/', httpOnly: true, secure: false, sameSite: 'Lax' };
	});
}

export async function launch({ mobile = false, video = null } = {}) {
	const viewport = mobile ? { width: 390, height: 844 } : { width: 1280, height: 800 };
	const browser = await chromium.launch({ headless: true });
	const ctx = await browser.newContext({
		viewport,
		deviceScaleFactor: 1,
		isMobile: mobile,
		hasTouch: mobile,
		locale: 'en-US',
		timezoneId: 'UTC',
		reducedMotion: 'no-preference',
		storageState: { cookies: await loginCookies(), origins: [] },
		...(video ? { recordVideo: { dir: video, size: viewport } } : {})
	});
	const page = await ctx.newPage();
	return { browser, ctx, page };
}
