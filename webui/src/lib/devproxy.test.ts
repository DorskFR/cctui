import { describe, expect, it } from 'vitest';
import { devProxy, stripSecure } from '../../vite.config';

describe('stripSecure', () => {
	it('drops Secure so a LAN dev origin can store the cookie over http', () => {
		expect(stripSecure('cctui_auth=t; Path=/; HttpOnly; SameSite=Lax; Max-Age=31536000; Secure')).toBe(
			'cctui_auth=t; Path=/; HttpOnly; SameSite=Lax; Max-Age=31536000'
		);
	});

	it('downgrades SameSite=None, which is only legal alongside Secure', () => {
		expect(stripSecure('a=b; SameSite=None; Secure')).toBe('a=b; SameSite=Lax');
	});

	it('leaves a cookie that was never Secure alone', () => {
		const plain = 'cctui_auth=t; Path=/; HttpOnly; SameSite=Lax';
		expect(stripSecure(plain)).toBe(plain);
	});

	it('does not maul a cookie whose value merely contains the word', () => {
		expect(stripSecure('x=Secure-ish; Path=/')).toBe('x=Secure-ish; Path=/');
	});
});

describe('devProxy', () => {
	it('is absent without CCTUI_PROXY, so a plain dev server stays plain', () => {
		expect(devProxy(undefined)).toBeUndefined();
	});

	it('presents the target origin upstream, since the allowlist has no LAN entry', () => {
		const cfg = devProxy('https://cctui.dorsk.dev')!;
		expect(cfg['/api'].headers).toEqual({ origin: 'https://cctui.dorsk.dev' });
		expect(cfg['/api'].ws).toBe(true);
		expect(cfg['/api'].secure).toBe(true);
	});

	it('rewrites every Set-Cookie on the response', () => {
		const cfg = devProxy('https://cctui.dorsk.dev')!;
		let onRes: ((r: { headers: Record<string, string | string[] | undefined> }) => void) | null =
			null;
		cfg['/api'].configure({
			on: (ev, fn) => {
				if (ev === 'proxyRes') onRes = fn;
			}
		});
		const res = { headers: { 'set-cookie': ['a=1; Secure', 'b=2; SameSite=None; Secure'] } };
		onRes!(res);
		expect(res.headers['set-cookie']).toEqual(['a=1', 'b=2; SameSite=Lax']);
	});
});
