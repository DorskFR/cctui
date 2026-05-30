/**
 * Minimal, safe-ish markdown → HTML for chat text bubbles. Mirrors what the
 * legacy HTML client rendered: escape first, then inline code, bold, italic,
 * links, and line breaks. Headings are flattened to bold. Used with {@html};
 * since we escape all input first, no raw HTML from the model survives.
 */
export function renderMarkdown(src: string): string {
	let s = escapeHtml(src);

	// fenced code blocks ```...```
	s = s.replace(/```([\s\S]*?)```/g, (_m, code) => `<pre class="md-pre">${code.trim()}</pre>`);
	// inline code
	s = s.replace(/`([^`]+)`/g, '<code class="md-code">$1</code>');
	// bold
	s = s.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');
	// italic (avoid touching ** already consumed)
	s = s.replace(/(^|[^*])\*([^*\n]+)\*/g, '$1<em>$2</em>');
	// links [text](url)
	s = s.replace(
		/\[([^\]]+)\]\((https?:\/\/[^\s)]+)\)/g,
		'<a href="$2" target="_blank" rel="noopener noreferrer">$1</a>'
	);
	// headings → bold line
	s = s.replace(/^#{1,6}\s+(.+)$/gm, '<strong>$1</strong>');
	// line breaks
	s = s.replace(/\n/g, '<br />');
	return s;
}

export function escapeHtml(s: string): string {
	return s
		.replace(/&/g, '&amp;')
		.replace(/</g, '&lt;')
		.replace(/>/g, '&gt;')
		.replace(/"/g, '&quot;');
}

export function prettyJson(v: unknown): string {
	try {
		return JSON.stringify(v, null, 2);
	} catch {
		return String(v);
	}
}
