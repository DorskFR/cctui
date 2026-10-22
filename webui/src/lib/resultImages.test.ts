import { describe, expect, it } from 'vitest';
import type { AgentEvent } from '@bindings/AgentEvent';
import { allFilter } from '$lib/components/organisms/conversation/filters';
import { toLines, type LineBuildCtx } from '$lib/components/organisms/conversation/lines';
import {
	parseResultImages,
	renderResultImages,
	resultDisplayText,
	resultImageLabel
} from './resultImages';

const HASH = 'ab'.repeat(32);
const PNG_B64 = 'iVBORw0KGgoAAAANSUhEUgAA'.repeat(40);

const blobRef = JSON.stringify([
	{ type: 'text', text: 'Screenshot saved' },
	{
		type: 'image',
		source: { type: 'cctui-blob', blob_id: HASH, size: 300_000, media_type: 'image/png' }
	}
]);
const inline = JSON.stringify([
	{ type: 'image', source: { type: 'base64', media_type: 'image/png', data: PNG_B64 } }
]);

describe('parseResultImages', () => {
	it('splits a blob reference from the text', () => {
		const parsed = parseResultImages(blobRef);
		expect(parsed?.text).toBe('Screenshot saved');
		expect(resultDisplayText(blobRef)).toBe('Screenshot saved\n\n[image image/png 293 KB]');
		expect(parsed?.images).toEqual([
			{ kind: 'blob', hash: HASH, mediaType: 'image/png', size: 300_000 }
		]);
	});

	it('labels an image-only result instead of showing base64', () => {
		const parsed = parseResultImages(inline);
		expect(parsed?.images[0]).toMatchObject({ kind: 'inline', mediaType: 'image/png' });
		expect(parsed?.text).toBe('[image image/png 720 B]');
		expect(resultDisplayText(inline)).not.toContain(PNG_B64.slice(0, 40));
	});

	it('leaves anything else to render as is', () => {
		expect(parseResultImages('plain output')).toBeNull();
		expect(parseResultImages('[{"type":"text","text":"no image"}]')).toBeNull();
		expect(parseResultImages('[{"type":"tool_reference"},{"type":"image"}]')).toBeNull();
		expect(resultDisplayText('[not json "image"')).toBe('[not json "image"');
	});

	it('labels what cannot be drawn: unsafe types, redacted data, opaque blobs', () => {
		const svg = JSON.stringify([
			{ type: 'image', source: { type: 'base64', media_type: 'image/svg+xml', data: 'PHN2Zz4=' } }
		]);
		expect(parseResultImages(svg)).toEqual({
			text: '[image image/svg+xml, unreadable]',
			images: [],
			labelled: '[image image/svg+xml, unreadable]'
		});
		const redacted = `${PNG_B64.slice(0, 100)}[REDACTED:square_token]${PNG_B64.slice(100)}`;
		const inlineRedacted = JSON.stringify([
			{ type: 'text', text: 'shot' },
			{ type: 'image', source: { type: 'base64', media_type: 'image/png', data: redacted } }
		]);
		const parsed = parseResultImages(inlineRedacted);
		expect(parsed?.text).toBe('shot\n\n[image image/png, unreadable]');
		expect(parsed?.images).toEqual([]);
		const opaque = JSON.stringify([
			{
				type: 'image',
				source: {
					type: 'cctui-blob',
					blob_id: HASH,
					size: 2048,
					media_type: 'image/png',
					encoding: 'base64'
				}
			}
		]);
		expect(parseResultImages(opaque)?.text).toBe('[image image/png 2 KB, unreadable]');
		expect(renderResultImages(parseResultImages(opaque)!.images, 's')).toBe('');
	});

	it('refuses malformed references', () => {
		const badHash = JSON.stringify([
			{ type: 'image', source: { type: 'cctui-blob', blob_id: '../../etc/passwd' } }
		]);
		expect(parseResultImages(badHash)).toBeNull();
	});
});

describe('renderResultImages', () => {
	it('points a reference at the session blob route and inlines base64 as data', () => {
		const html = renderResultImages(
			[...parseResultImages(blobRef)!.images, ...parseResultImages(inline)!.images],
			'sess-1'
		);
		expect(html).toContain(`src="/api/v1/sessions/sess-1/blobs/${HASH}"`);
		expect(html).toContain('src="data:image/png;base64,iVBOR');
		expect(html.match(/class="md-img"/g)).toHaveLength(2);
	});

	it('labels sizes', () => {
		expect(
			resultImageLabel({ kind: 'blob', hash: HASH, mediaType: null, size: null })
		).toBe('[image image]');
	});
});

describe('tool_result lines', () => {
	const ctx: LineBuildCtx = {
		visible: (c) => allFilter(true)[c],
		renderMarkdown: (s) => `<p>${s}</p>`,
		renderCode: (text) => `<code>${text}</code>`,
		prettyJson: true,
		prettyDiff: true
	};
	const result = (output_summary: string): AgentEvent => ({
		type: 'tool_result',
		tool: 'Read',
		output_summary,
		kind: null,
		tool_use_id: 'toolu_1',
		error: false,
		ts: 1,
		seq: 7
	});

	it('carry the images apart from the text', () => {
		const [line] = toLines(result(blobRef), ctx);
		expect(line.text).toBe('Screenshot saved');
		expect(line.htmlCode).toBe('<code>Screenshot saved</code>');
		expect(line.images).toHaveLength(1);
	});

	it('keep a plain result unchanged', () => {
		const [line] = toLines(result('ok'), ctx);
		expect(line.text).toBe('ok');
		expect(line.images).toBeUndefined();
	});
});
