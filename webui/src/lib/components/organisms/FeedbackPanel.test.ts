// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';

const h = vi.hoisted(() => ({ imported: 0 }));
vi.mock('@dorsk/hmm', async () => {
	h.imported++;
	const { default: Hmm } = await import('./FeedbackPanel.stub.svelte');
	return { Hmm };
});
vi.stubGlobal('__CLIENT_VERSION__', '0.31.0-test');

import { feedbackConfig } from '$lib/config';
import FeedbackPanel from './FeedbackPanel.svelte';
import { received } from './FeedbackPanel.stub.svelte';

const setConfig = (cfg: Window['CCTUI_CONFIG']) => Object.assign(window, { CCTUI_CONFIG: cfg });
const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

let comp: ReturnType<typeof mount> | null = null;
function render(serverVersion: string | undefined = '0.31.0', route: string | null = '/sessions') {
	comp = mount(FeedbackPanel, {
		target: document.body,
		props: { serverVersion: () => serverVersion, route: () => route }
	});
	flushSync();
}

afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
	received.length = 0;
	delete window.CCTUI_CONFIG;
});

describe('feedbackConfig', () => {
	it('is off without an endpoint, and for a blank one', () => {
		setConfig({ apiBase: '' });
		expect(feedbackConfig()).toBeNull();
		setConfig({ feedback: { endpoint: '  ', projectKey: 'x' } });
		expect(feedbackConfig()).toBeNull();
	});

	it('defaults the project key to cctui', () => {
		setConfig({ feedback: { endpoint: 'https://hmm.test' } });
		expect(feedbackConfig()).toEqual({ endpoint: 'https://hmm.test', projectKey: 'cctui' });
		setConfig({ feedback: { endpoint: 'https://hmm.test', projectKey: 'staging' } });
		expect(feedbackConfig()?.projectKey).toBe('staging');
	});
});

describe('FeedbackPanel', () => {
	it('never imports @dorsk/hmm and renders no launcher without feedback.endpoint', async () => {
		setConfig({ apiBase: '' });
		render();
		await settle();
		flushSync();
		expect(h.imported).toBe(0);
		expect(received).toHaveLength(0);
		expect(document.body.querySelector('[data-hmm-stub]')).toBeNull();
		expect(document.body.innerHTML.trim()).toBe('');
	});

	it('renders the panel with the configured endpoint and project key', async () => {
		setConfig({ feedback: { endpoint: 'https://hmm.test' } });
		render();
		await vi.waitFor(() => {
			flushSync();
			expect(received).toHaveLength(1);
		});
		expect(h.imported).toBe(1);
		expect(document.body.querySelector('[data-hmm-stub]')).not.toBeNull();
		const props = received[0];
		expect(props.endpoint).toBe('https://hmm.test');
		expect(props.projectKey).toBe('cctui');
		expect(props.position).toBe('bottom-left');
		expect((props.labels as { launcher: string }).launcher).toBe('Feedback');
	});

	it('reports only the versions and the route as context', async () => {
		setConfig({ feedback: { endpoint: 'https://hmm.test', projectKey: 'cctui' } });
		render('0.31.0', '/sessions/[id]');
		await vi.waitFor(() => {
			flushSync();
			expect(received).toHaveLength(1);
		});
		const context = received[0].context as () => Record<string, string>;
		expect(context()).toEqual({
			webuiVersion: '0.31.0-test',
			serverVersion: '0.31.0',
			route: '/sessions/[id]'
		});
	});

	it('leaves the server version out until it is known', async () => {
		setConfig({ feedback: { endpoint: 'https://hmm.test' } });
		render(undefined, null);
		await vi.waitFor(() => {
			flushSync();
			expect(received).toHaveLength(1);
		});
		const context = received[0].context as () => Record<string, string>;
		expect(context()).toEqual({ webuiVersion: '0.31.0-test' });
	});
});
