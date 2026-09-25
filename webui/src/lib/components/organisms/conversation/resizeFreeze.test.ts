// @vitest-environment happy-dom
import { afterEach, describe, expect, it } from 'vitest';
import { freezeWidthDuringResize } from './resizeFreeze';

const setup = () => {
	const panel = document.createElement('div');
	const content = document.createElement('div');
	const drawer = document.createElement('div');
	const sep = document.createElement('div');
	sep.setAttribute('role', 'separator');
	content.appendChild(drawer);
	panel.append(content, sep);
	document.body.appendChild(panel);
	drawer.getBoundingClientRect = () => ({ width: 640 }) as DOMRect;
	const action = freezeWidthDuringResize(drawer);
	return { panel, drawer, sep, action };
};

const down = (el: Element) =>
	el.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }));

describe('freezeWidthDuringResize', () => {
	afterEach(() => {
		document.body.innerHTML = '';
	});

	it('pins the width from pointerdown on the separator until pointerup', () => {
		const { drawer, sep, action } = setup();
		down(sep);
		expect(drawer.style.width).toBe('640px');
		dispatchEvent(new PointerEvent('pointerup'));
		expect(drawer.style.width).toBe('');
		action.destroy();
	});

	it('releases on pointercancel', () => {
		const { drawer, sep, action } = setup();
		down(sep);
		dispatchEvent(new PointerEvent('pointercancel'));
		expect(drawer.style.width).toBe('');
		action.destroy();
	});

	it('ignores pointers elsewhere and unrelated separators', () => {
		const { drawer, action } = setup();
		down(drawer);
		const other = document.createElement('div');
		other.setAttribute('role', 'separator');
		document.body.appendChild(other);
		down(other);
		expect(drawer.style.width).toBe('');
		action.destroy();
	});
});
