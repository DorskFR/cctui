// @vitest-environment happy-dom
import { afterEach, describe, expect, it } from 'vitest';
import { guardEscape } from './escapeGuard';

afterEach(() => {
	document.body.innerHTML = '';
});

function press(target: Element, panel: Element, key = 'Escape') {
	const e = new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true });
	panel.addEventListener('keydown', guardEscape as EventListener, { once: true });
	target.dispatchEvent(e);
	return e.defaultPrevented;
}

function host() {
	document.body.innerHTML = `
		<div role="dialog" id="panel">
			<button id="btn"></button>
			<input id="rename" />
			<div role="dialog" id="nested"><button id="inner"></button></div>
		</div>`;
	return document.getElementById('panel') as HTMLElement;
}

describe('guardEscape', () => {
	it('lets Escape from the panel itself through', () => {
		const panel = host();
		expect(press(document.getElementById('btn')!, panel)).toBe(false);
	});

	it('ignores other keys', () => {
		const panel = host();
		expect(press(document.getElementById('rename')!, panel, 'Enter')).toBe(false);
	});

	it('swallows Escape from an input so the rename keeps it', () => {
		const panel = host();
		expect(press(document.getElementById('rename')!, panel)).toBe(true);
	});

	it('swallows Escape from a nested dialog', () => {
		const panel = host();
		expect(press(document.getElementById('inner')!, panel)).toBe(true);
	});
});
