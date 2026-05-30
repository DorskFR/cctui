/**
 * Svelte action: grow a <textarea> with its content up to a max height, then
 * scroll. Pass the bound value so it re-measures on programmatic changes
 * (drafts loading, clearing, etc.).
 *
 * Usage: <textarea use:autoresize={value} ...></textarea>
 */
export function autoresize(node: HTMLTextAreaElement, _value?: string) {
	const resize = () => {
		node.style.height = 'auto';
		node.style.height = `${node.scrollHeight}px`;
	};
	resize();
	node.addEventListener('input', resize);
	return {
		update() {
			// re-measure when the bound value changes from outside (e.g. cleared)
			resize();
		},
		destroy() {
			node.removeEventListener('input', resize);
		}
	};
}
