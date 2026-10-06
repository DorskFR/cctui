// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';

vi.mock('$app/environment', () => ({ browser: true }));

import {
	attemptOpen,
	classify,
	deniedRoots,
	installFileViewer,
	previewFile,
	previewable,
	linkedFileHref,
	refusalMessage
} from './fileviewer';

describe('fileviewer classify', () => {
	it('routes by base content type', () => {
		expect(classify('image/png')).toBe('image');
		expect(classify('text/markdown; charset=utf-8')).toBe('markdown');
		expect(classify('text/plain; charset=utf-8')).toBe('text');
		expect(classify('application/json; charset=utf-8')).toBe('text');
		expect(classify('application/pdf')).toBe('download');
		expect(classify('application/octet-stream')).toBe('download');
		expect(classify('text/html')).toBe('download');
		expect(classify(null)).toBe('download');
	});
});

describe('fileviewer refusalMessage', () => {
	it('names the file and distinguishes the refusal kinds', () => {
		const tooLarge = refusalMessage(413, 'big.zip');
		const denied = refusalMessage(403, 'x.md');
		const missing = refusalMessage(404, 'x.md');
		const offline = refusalMessage(503, 'x.md');
		const other = refusalMessage(500, 'x.md');
		for (const t of [tooLarge, denied, missing, offline, other]) expect(t).toMatch(/x\.md|big\.zip/);
		expect(new Set([tooLarge, denied, missing, offline, other]).size).toBe(5);
		expect(other).toContain('500');
	});

	it('gives blob 404, fs 404 and fs 503 three distinct messages', () => {
		const blobMissing = refusalMessage(404, 'shot.png', 'blob');
		const fsMissing = refusalMessage(404, 'shot.png', 'machine');
		const fsOffline = refusalMessage(503, 'shot.png', 'machine');
		expect(new Set([blobMissing, fsMissing, fsOffline]).size).toBe(3);
		for (const t of [blobMissing, fsMissing, fsOffline]) expect(t).toContain('shot.png');
	});

	it('never blames the machine for a blob-store refusal', () => {
		for (const status of [400, 404, 500, 503]) {
			expect(refusalMessage(status, 'shot.png', 'blob')).not.toMatch(/machine/i);
		}
	});

	it('names the roots a denial was checked against, when the daemon listed them', () => {
		const detail = '/home/gtax/.claude/jobs/cdfadc1d/tmp/x.md is outside the allowed roots: /tmp, /srv/app';
		expect(deniedRoots(detail)).toEqual(['/tmp', '/srv/app']);
		expect(deniedRoots('path was not linked in this session')).toEqual([]);

		const withRoots = refusalMessage(403, 'x.md', 'machine', detail);
		expect(withRoots).toContain('/tmp');
		expect(withRoots).toContain('/srv/app');
		expect(withRoots).not.toBe(refusalMessage(403, 'x.md', 'machine'));
	});

	it('prefers the structured folder list over parsing the prose', () => {
		const detail = '/x/note.md is outside the allowed roots: /tmp';
		expect(deniedRoots(detail, ['/srv/app', '/var/data'])).toEqual(['/srv/app', '/var/data']);
		expect(deniedRoots(detail)).toEqual(['/tmp']);
		expect(deniedRoots('', ['/srv/app'])).toEqual(['/srv/app']);
		expect(deniedRoots('path was not linked in this session', [])).toEqual([]);

		const worded = refusalMessage(403, 'x.md', 'machine', 'nothing parseable here');
		const structured = refusalMessage(403, 'x.md', 'machine', 'nothing parseable here', [
			'/srv/app'
		]);
		expect(structured).toContain('/srv/app');
		expect(structured).not.toBe(worded);
	});

	it('calls a network failure a network failure on either source', () => {
		for (const source of ['machine', 'blob'] as const) {
			expect(refusalMessage(0, 'x.md', source)).toContain('network');
		}
	});
});

describe('routing a linked path to the machine that owns it', () => {
	afterEach(() => vi.unstubAllGlobals());

	const HREF = '/api/v1/machines/m1/fs/file?path=%2Fx%2Fnote.md&session_id=s1';

	it('rewrites only a machine file href into the session linked-file route', () => {
		expect(linkedFileHref(HREF)).toBe('/api/v1/sessions/s1/linked-file?path=%2Fx%2Fnote.md');
		expect(linkedFileHref('/api/v1/sessions/s1/blobs/abc')).toBeNull();
	});

	it('retries the read through the server proxy after a refusal', async () => {
		const seen: string[] = [];
		vi.stubGlobal(
			'fetch',
			vi.fn(async (url: string) => {
				seen.push(url);
				if (url.includes('/linked-file?'))
					return new Response('hello', {
						status: 200,
						headers: { 'content-type': 'text/plain' }
					});
				return new Response(JSON.stringify({ error: 'path was not linked in this session' }), {
					status: 403
				});
			})
		);
		URL.createObjectURL = vi.fn(() => 'blob:stub');
		URL.revokeObjectURL = vi.fn();
		expect(await attemptOpen(HREF, 'note.md')).toBeNull();
		expect(seen[0]).toBe(HREF);
		expect(seen[1]).toContain('/api/v1/sessions/s1/linked-file?path=%2Fx%2Fnote.md');
		expect(seen).toHaveLength(2);
		document.body.innerHTML = '';
	});

	it('shows the owning daemon denial with its structured folders', async () => {
		vi.stubGlobal(
			'fetch',
			vi.fn(async (url: string) =>
				url.includes('/linked-file?')
					? new Response(
							JSON.stringify({ error: 'outside the allowed roots', allowed_folders: ['/srv'] }),
							{ status: 403 }
						)
					: new Response(JSON.stringify({ error: 'not linked' }), { status: 403 })
			)
		);
		const refusal = await attemptOpen(HREF, 'note.md');
		expect(refusal?.status).toBe(403);
		expect(refusal?.allowedFolders).toEqual(['/srv']);
	});

	it('keeps the original refusal when no other session owns the path', async () => {
		vi.stubGlobal(
			'fetch',
			vi.fn(async (url: string) =>
				url.includes('/linked-file?')
					? new Response(JSON.stringify({ error: 'not linked' }), { status: 404 })
					: new Response(
							JSON.stringify({
								error: '/x/note.md is outside the allowed roots: /tmp',
								allowed_folders: ['/tmp']
							}),
							{ status: 403 }
						)
			)
		);
		const refusal = await attemptOpen(HREF, 'note.md');
		expect(refusal?.status).toBe(403);
		expect(refusal?.allowedFolders).toEqual(['/tmp']);
	});

	it('does not chase an owner for a too-large or offline refusal', async () => {
		const seen: string[] = [];
		vi.stubGlobal(
			'fetch',
			vi.fn(async (url: string) => {
				seen.push(url);
				return new Response(JSON.stringify({ error: 'too big' }), { status: 413 });
			})
		);
		expect((await attemptOpen(HREF, 'note.md'))?.status).toBe(413);
		expect(seen).toEqual([HREF]);
	});
});

describe('fileviewer inline refusals', () => {
	afterEach(() => {
		document.body.innerHTML = '';
		vi.unstubAllGlobals();
	});

	function link(): HTMLAnchorElement {
		URL.createObjectURL = vi.fn(() => 'blob:stub');
		URL.revokeObjectURL = vi.fn();
		document.body.innerHTML =
			'<a class="md-file" href="/api/v1/machines/m/fs/file?path=/x/note.md&session_id=s" data-file-name="note.md">/x/note.md</a>';
		installFileViewer();
		return document.querySelector('a.md-file') as HTMLAnchorElement;
	}

	function click(a: HTMLAnchorElement): void {
		a.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true, button: 0 }));
	}

	it('shows the refusal next to the link instead of navigating to the body', async () => {
		const detail = '/x/note.md is outside the allowed roots: /tmp, /home/gtax/.claude/jobs';
		vi.stubGlobal(
			'fetch',
			vi.fn(async () => new Response(JSON.stringify({ error: detail }), { status: 403 }))
		);
		const a = link();
		click(a);

		const err = await vi.waitFor(() => {
			const el = document.querySelector('.md-file-error');
			expect(el).not.toBeNull();
			return el;
		});
		expect(err?.textContent).toContain('note.md');
		expect(err?.textContent).toContain('/home/gtax/.claude/jobs');
		expect(a.nextElementSibling).toBe(err);
	});

	it('clears a previous refusal when a retry succeeds', async () => {
		// Keyed by URL, not by call order: a refused read is retried through
		// the linked-file route.
		let reads = 0;
		vi.stubGlobal(
			'fetch',
			vi.fn(async (url: string) => {
				if (url.includes('/linked-file?'))
					return new Response(JSON.stringify({ error: 'not linked' }), { status: 404 });
				reads += 1;
				return reads === 1
					? new Response(JSON.stringify({ error: 'nope' }), { status: 404 })
					: new Response('hello', { status: 200, headers: { 'content-type': 'text/plain' } });
			})
		);
		const a = link();

		click(a);
		await vi.waitFor(() => expect(document.querySelector('.md-file-error')).not.toBeNull());

		click(a);
		await vi.waitFor(() => expect(document.querySelector('.md-file-error')).toBeNull());
	});
});

describe('fileviewer previewFile', () => {
	afterEach(() => {
		document.body.innerHTML = '';
	});

	it('only previews what the overlay can show', () => {
		expect(previewable('image/png')).toBe(true);
		expect(previewable('text/plain')).toBe(true);
		expect(previewable('text/markdown')).toBe(true);
		expect(previewable('application/pdf')).toBe(false);
		expect(previewable('')).toBe(false);
	});

	it('opens a local image in the lightbox overlay and revokes its URL on close', async () => {
		URL.createObjectURL = vi.fn(() => 'blob:local');
		const revoke = vi.fn();
		URL.revokeObjectURL = revoke;
		await previewFile(new File([new Uint8Array(4)], 'shot 12.21.15.png', { type: 'image/png' }));
		const overlay = document.querySelector('.md-fileviewer');
		expect(overlay?.getAttribute('aria-label')).toBe('shot 12.21.15.png');
		expect(overlay?.querySelector('img.md-lightbox-img')?.getAttribute('src')).toBe('blob:local');
		document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }));
		expect(document.querySelector('.md-fileviewer')).toBeNull();
		expect(revoke).toHaveBeenCalledWith('blob:local');
	});

	it('opens a local text file as preformatted text', async () => {
		URL.createObjectURL = vi.fn(() => 'blob:text');
		URL.revokeObjectURL = vi.fn();
		await previewFile(new File(['hello\nworld'], 'paste-1.txt', { type: 'text/plain' }));
		expect(document.querySelector('.md-fileviewer-pre')?.textContent).toBe('hello\nworld');
	});
});
