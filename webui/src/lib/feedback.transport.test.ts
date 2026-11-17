// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { type Envelope, httpTransport, SendError } from '@dorsk/hmm';
import { feedbackConfig } from './config';

const envelope = { v: 1, id: 'report-1', kind: 'feedback', comment: 'hello' } as unknown as Envelope;

function receiver(status: number) {
	const fetcher = vi.fn(async (_url: string, _init: RequestInit) => ({ ok: status < 300, status }));
	vi.stubGlobal('fetch', fetcher);
	return fetcher;
}

function transportFromConfig() {
	const cfg = feedbackConfig();
	if (!cfg) throw new Error('feedback is not configured');
	return httpTransport(cfg.endpoint, cfg.projectKey);
}

afterEach(() => {
	vi.unstubAllGlobals();
	delete window.CCTUI_CONFIG;
});

describe('feedback delivery through @dorsk/hmm', () => {
	it('posts the report to <endpoint>/v1/reports with x-hmm-key set to the project key', async () => {
		Object.assign(window, { CCTUI_CONFIG: { feedback: { endpoint: 'https://hmm.test/' } } });
		const fetcher = receiver(202);
		await transportFromConfig()(envelope);
		expect(fetcher).toHaveBeenCalledTimes(1);
		const [url, init] = fetcher.mock.calls[0];
		expect(url).toBe('https://hmm.test/v1/reports');
		expect(init.method).toBe('POST');
		expect(init.credentials).toBe('omit');
		expect(init.headers).toEqual({ 'content-type': 'application/json', 'x-hmm-key': 'cctui' });
		expect(JSON.parse(init.body as string)).toEqual(envelope);
	});

	it('marks a server error retryable and a rejected report permanent', async () => {
		Object.assign(window, { CCTUI_CONFIG: { feedback: { endpoint: 'https://hmm.test', projectKey: 'cctui' } } });
		receiver(503);
		const busy = await transportFromConfig()(envelope).catch((e: unknown) => e);
		expect(busy).toBeInstanceOf(SendError);
		expect((busy as SendError).retryable).toBe(true);
		receiver(400);
		const rejected = await transportFromConfig()(envelope).catch((e: unknown) => e);
		expect((rejected as SendError).retryable).toBe(false);
	});
});
