import { describe, expect, it } from 'vitest';
import { safeHref } from './safeHref';

describe('safeHref', () => {
	it('keeps http(s) and same-origin relative paths', () => {
		expect(safeHref('https://ok.example/x')).toBe('https://ok.example/x');
		expect(safeHref('http://ok.example')).toBe('http://ok.example');
		expect(safeHref('/api/v1/machines/m/fs/file?path=%2Ftmp%2Fx.png')).toBe(
			'/api/v1/machines/m/fs/file?path=%2Ftmp%2Fx.png'
		);
	});

	it('refuses every other scheme, protocol-relative and empty', () => {
		for (const bad of [
			'javascript:alert(1)',
			'file:///etc/passwd',
			'file://host/x',
			'data:text/html,x',
			'vbscript:x',
			'//evil.example/x',
			'relative/path.md',
			'~/x.md',
			'',
			null,
			undefined
		]) {
			expect(safeHref(bad), String(bad)).toBeUndefined();
		}
	});
});
