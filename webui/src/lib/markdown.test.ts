import { describe, expect, it } from 'vitest';
import { renderMarkdown } from './markdown';

describe('renderMarkdown cctui-img rendering', () => {
	const sid = 'sess-abc';

	it('renders a cctui-img marker as a session-scoped <img> when a sessionId is given', () => {
		const html = renderMarkdown('here ![a shot](cctui-img://img-123)', { sessionId: sid });
		expect(html).toContain(
			'<img class="md-img" src="/api/v1/sessions/sess-abc/images/img-123"',
		);
		expect(html).toContain('alt="a shot"');
		expect(html).toContain('data-lightbox="/api/v1/sessions/sess-abc/images/img-123"');
	});

	it('leaves the marker as escaped text when no sessionId is in scope (export/preview)', () => {
		const html = renderMarkdown('![a shot](cctui-img://img-123)');
		expect(html).not.toContain('<img');
		expect(html).toContain('cctui-img://img-123');
	});

	it('does NOT turn remote/model-authored image URLs into <img> (XSS/track guard)', () => {
		const html = renderMarkdown('![evil](https://tracker.example/x.png)', { sessionId: sid });
		expect(html).not.toContain('<img');
	});

	it('escapes a hostile alt attribute so it cannot break out', () => {
		const html = renderMarkdown('![" onerror=alert(1) x](cctui-img://id1)', {
			sessionId: sid,
		});
		// The double-quote is entity-escaped, so the attribute can't be closed
		// early — the payload is inert text inside alt=".
		expect(html).toContain('alt="&quot; onerror=alert(1) x"');
		expect(html).not.toContain('alt="" onerror=alert(1)');
	});

	it('renders multiple markers', () => {
		const html = renderMarkdown('![a](cctui-img://one) ![b](cctui-img://two)', {
			sessionId: sid,
		});
		expect(html).toContain('/images/one');
		expect(html).toContain('/images/two');
	});
});

describe('renderMarkdown local path links', () => {
	const opts = { machineId: 'mach-1', sessionId: 'sess-1' };
	const anchors = (html: string) => {
		const host = document.createElement('div');
		host.innerHTML = html;
		return [...host.querySelectorAll('a')].map((a) => ({
			href: a.getAttribute('href'),
			text: a.textContent,
			cls: a.className,
			name: a.getAttribute('data-file-name')
		}));
	};

	it('links absolute and ~ paths to the machine read-file route with the session', () => {
		const got = anchors(renderMarkdown('see /home/u/out/report.md and ~/shots/x.png.', opts));
		expect(got).toEqual([
			{
				href: '/api/v1/machines/mach-1/fs/file?path=%2Fhome%2Fu%2Fout%2Freport.md&session_id=sess-1',
				text: '/home/u/out/report.md',
				cls: 'md-file',
				name: 'report.md'
			},
			{
				href: '/api/v1/machines/mach-1/fs/file?path=~%2Fshots%2Fx.png&session_id=sess-1',
				text: '~/shots/x.png',
				cls: 'md-file',
				name: 'x.png'
			}
		]);
	});

	it('links paths inside inline code, parentheses and table cells', () => {
		expect(anchors(renderMarkdown('run `/tmp/a.sh` (see /tmp/b.txt)', opts)).map((a) => a.text)).toEqual([
			'/tmp/a.sh',
			'/tmp/b.txt'
		]);
		const table = '| f | n |\n|---|---|\n| /tmp/c.log | 1 |';
		expect(anchors(renderMarkdown(table, opts)).map((a) => a.text)).toEqual(['/tmp/c.log']);
	});

	it('does not link without a machine id, inside URLs, code blocks, or extension-less paths', () => {
		expect(anchors(renderMarkdown('see /tmp/x.png'))).toEqual([]);
		expect(anchors(renderMarkdown('see /tmp/x.png', { sessionId: 'sess-1' }))).toEqual([]);
		const url = anchors(renderMarkdown('https://ok.example/dir/file.md', opts));
		expect(url).toHaveLength(1);
		expect(url[0].href).toBe('https://ok.example/dir/file.md');
		expect(anchors(renderMarkdown('```\n/tmp/x.png\n```', opts))).toEqual([]);
		expect(anchors(renderMarkdown('cd /home/u/proj and /usr/bin', opts))).toEqual([]);
		expect(anchors(renderMarkdown('a/b.txt and 1/2.5', opts))).toEqual([]);
	});

	it('keeps trailing punctuation out of the path', () => {
		const got = anchors(renderMarkdown('open /tmp/x.md, then /tmp/y.md; done /tmp/z.md.', opts));
		expect(got.map((a) => a.text)).toEqual(['/tmp/x.md', '/tmp/y.md', '/tmp/z.md']);
	});
});
