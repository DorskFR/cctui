import { flushSync, mount, unmount, type Component } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import AppearanceSection from './AppearanceSection.svelte';
import ExecutionSection from './ExecutionSection.svelte';
import NotificationsSection from './NotificationsSection.svelte';
import PrivacySection from './PrivacySection.svelte';
import SessionsSection from './SessionsSection.svelte';

type Control = HTMLElement;

function accessibleName(el: Control): string {
	const aria = el.getAttribute('aria-label');
	if (aria?.trim()) return aria.trim();
	const labelledBy = el.getAttribute('aria-labelledby');
	if (labelledBy) {
		const text = labelledBy
			.split(/\s+/)
			.map((id) => document.getElementById(id)?.textContent ?? '')
			.join(' ')
			.trim();
		if (text) return text;
	}
	if (el.id) {
		const label = document.querySelector(`label[for="${CSS.escape(el.id)}"]`);
		if (label?.textContent?.trim()) return label.textContent.trim();
	}
	return el.closest('label')?.textContent?.trim() ?? '';
}

function controls(): Control[] {
	return Array.from(
		document.querySelectorAll<Control>('input, select, textarea, [role="switch"]')
	);
}

let component: ReturnType<typeof mount> | undefined;
let warn: ReturnType<typeof vi.spyOn>;

beforeEach(() => {
	warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
});

afterEach(() => {
	if (component) unmount(component);
	component = undefined;
	document.body.replaceChildren();
	warn.mockRestore();
});

const sections: [string, Component<Record<string, never>>][] = [
	['Appearance', AppearanceSection as Component<Record<string, never>>],
	['Sessions', SessionsSection as Component<Record<string, never>>],
	['Execution', ExecutionSection as Component<Record<string, never>>],
	['Privacy', PrivacySection as Component<Record<string, never>>],
	['Notifications', NotificationsSection as Component<Record<string, never>>]
];

describe('settings sections', () => {
	it.each(sections)('%s labels every control', (_name, Section) => {
		component = mount(Section, { target: document.body, props: {} });
		flushSync();

		const found = controls();
		expect(found.length).toBeGreaterThan(0);
		const unnamed = found
			.filter((el) => !accessibleName(el))
			.map((el) => `${el.tagName.toLowerCase()}[${el.getAttribute('placeholder') ?? ''}]`);
		expect(unnamed).toEqual([]);
		expect(
			warn.mock.calls.flat().filter((a: unknown) => String(a).includes('without an accessible label'))
		).toEqual([]);
	});

	it.each(sections)('%s gives each control a unique id', (_name, Section) => {
		component = mount(Section, { target: document.body, props: {} });
		flushSync();

		const ids = controls()
			.map((el) => el.id)
			.filter(Boolean);
		expect(ids).toEqual([...new Set(ids)]);
	});

	it('names the Appearance selects after their row label', () => {
		component = mount(AppearanceSection as Component<Record<string, never>>, {
			target: document.body,
			props: {}
		});
		flushSync();

		const names = Array.from(document.querySelectorAll<HTMLSelectElement>('select')).map(
			accessibleName
		);
		expect(names).toContain('Theme');
		expect(names).toContain('Interface language');
	});
});
