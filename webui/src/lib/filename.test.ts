import { describe, expect, it } from 'vitest';
import { shortFileName } from './filename';

const a = 'Screenshot 2026-10-03 at 12.21.15.png';
const b = 'Screenshot 2026-10-03 at 12.21.12.png';

describe('shortFileName', () => {
	it('keeps the distinguishing tail of look-alike screenshot names', () => {
		const sa = shortFileName(a, 20);
		const sb = shortFileName(b, 20);
		expect(sa.length).toBeLessThanOrEqual(20);
		expect(sa).toMatch(/…/);
		expect(sa.endsWith('12.21.15.png')).toBe(true);
		expect(sb.endsWith('12.21.12.png')).toBe(true);
		expect(sa).not.toBe(sb);
	});

	it('leaves a short name alone', () => {
		expect(shortFileName('a.png')).toBe('a.png');
	});

	it('counts code points, not UTF-16 units', () => {
		expect(shortFileName('📎📎📎📎📎📎📎📎📎📎.png', 8)).toBe('📎📎…📎.png');
	});
});
