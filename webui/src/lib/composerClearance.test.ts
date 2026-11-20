// @vitest-environment happy-dom
import { afterEach, describe, expect, it } from 'vitest';
import { clearanceOf, composerClearance } from './composerClearance';

const rect = (top: number, height: number) => ({ top, height, bottom: top + height });

describe('clearanceOf', () => {
	it('is the distance from the composer top to the viewport bottom', () => {
		expect(clearanceOf(rect(780, 120), 915)).toBe(135);
	});

	it('ignores hidden composers and ones in the upper half', () => {
		expect(clearanceOf(rect(0, 0), 915)).toBe(0);
		expect(clearanceOf(rect(200, 120), 915)).toBe(0);
	});
});

describe('composerClearance', () => {
	const clear = () => document.documentElement.style.getPropertyValue('--composer-clear');

	afterEach(() => document.body.replaceChildren());

	it('publishes the highest clearance and clears it once every composer is gone', () => {
		const at = (top: number) => {
			const el = document.createElement('div');
			el.getBoundingClientRect = () => new DOMRect(0, top, 10, 100);
			document.body.append(el);
			return composerClearance(el) as () => void;
		};
		const low = at(innerHeight - 100);
		const high = at(innerHeight - 150);
		expect(clear()).toBe('150px');
		high();
		expect(clear()).toBe('100px');
		low();
		expect(clear()).toBe('');
	});
});
