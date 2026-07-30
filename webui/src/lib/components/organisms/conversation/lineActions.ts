// Per-message line actions (copy-as-Markdown, save-as-PNG) for the conversation
// drawer, extracted from ConversationDrawer with no behavior change.
import { errMessage } from '$lib/api';
import { toasts } from '$lib/toast.svelte';
import { copyText } from '$lib/clipboard';
import { lineMarkdown } from './format';
import type { Line } from './types';
import { m } from '$lib/paraglide/messages';

export async function copyLineMarkdown(ln: Line) {
	await copyText(lineMarkdown(ln), m.conversation_copied_markdown());
}

// Save a single message as a PNG, rendered with the current theme.
// We snapshot the live `.line` node (so theme colors come for free), filtering
// out the hover action buttons, and bake the page background in so transparent
// bubbles read correctly. html-to-image is loaded on demand to keep it out of
// the main bundle.
export async function saveLineImage(e: MouseEvent, ln: Line) {
	const node = (e.currentTarget as HTMLElement).closest('.line') as HTMLElement | null;
	if (!node) return;
	try {
		const bg = getComputedStyle(document.body).getPropertyValue('--bg').trim() || '#1e1e1e';
		const { toPng } = await import('html-to-image');
		// Capture the ACTUAL on-screen node in place — do NOT clone it off into a
		// detached/hidden element first. html-to-image rasterises by serialising the
		// node into an SVG <foreignObject>, reading each element's *computed* style;
		// on a clone moved out of the live cascade the `color-mix()`/CSS-variable
		// theme tokens (and any `opacity:0` on the capture root) resolve to nothing,
		// yielding a blank PNG. The visible node is already laid out with fonts
		// loaded and every var/color-mix resolved, so capturing it directly paints
		// the full content. We only hide the hover action buttons for the duration
		// of the capture, then restore them.
		const actions = node.querySelector<HTMLElement>('.line-actions');
		const prevActionsDisplay = actions?.style.display ?? '';
		if (actions) actions.style.display = 'none';
		// Make sure fonts are ready so glyphs paint rather than render as blank
		// boxes, and let the layout settle (the hidden action buttons reflow).
		if (document.fonts?.ready) await document.fonts.ready;
		await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
		let dataUrl: string;
		try {
			const rect = node.getBoundingClientRect();
			// The capture adds a uniform PADDING with box-sizing:border-box, which
			// shrinks the content box by 2×pad — so the content (laid out at its full
			// width) was clipped on the right by ~pad. Add the padding to the
			// requested dimensions so the content keeps its full width and the padding
			// sits OUTSIDE it.
			const pad = 16;
			// scrollHeight/Width captures the full content even if the node is inside a
			// scroll container, so nothing is clipped vertically.
			const contentW = Math.ceil(Math.max(rect.width, node.scrollWidth));
			const contentH = Math.ceil(Math.max(rect.height, node.scrollHeight));
			dataUrl = await toPng(node, {
				pixelRatio: 2,
				cacheBust: true,
				backgroundColor: bg,
				width: contentW + pad * 2,
				height: contentH + pad * 2,
				style: { margin: '0', padding: `${pad}px`, boxSizing: 'border-box', background: bg }
			});
		} finally {
			if (actions) actions.style.display = prevActionsDisplay;
		}
		const a = document.createElement('a');
		a.download = `cctui-message-${ln.ts}.png`;
		a.href = dataUrl;
		a.click();
		toasts.ok(m.conversation_saved_image());
	} catch (err) {
		toasts.err(m.conversation_image_export_failed({ message: errMessage(err) }));
	}
}
