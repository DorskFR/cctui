import { describe, expect, it } from 'vitest';
import { renderMarkdown } from './markdown';

describe('renderMarkdown cctui-img rendering (CCT-566)', () => {
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
