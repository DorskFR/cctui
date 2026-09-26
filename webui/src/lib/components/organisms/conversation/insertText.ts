/** Splice a block into a draft at the caret, kept apart from surrounding text
 *  by a blank line, with the caret left on the line after it. */
export function insertBlock(current: string, block: string, caret?: number): { value: string; caret: number } {
	const at = Math.max(0, Math.min(current.length, caret ?? current.length));
	const before = current.slice(0, at);
	const after = current.slice(at);
	const pre = before === '' ? '' : before.endsWith('\n\n') ? '' : before.endsWith('\n') ? '\n' : '\n\n';
	const post = after === '' ? '\n' : after.startsWith('\n\n') ? '' : after.startsWith('\n') ? '\n' : '\n\n';
	const head = before + pre + block + post;
	return { value: head + after, caret: head.length };
}
