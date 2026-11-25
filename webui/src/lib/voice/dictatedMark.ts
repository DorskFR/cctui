/** Mirrors `dictated_prompt` in `crates/cctui-proto/src/injected.rs`, so the
 *  optimistic echo of a dictated send reads like the turn the server stores. */
const NOTE =
	'The user dictated this message: it went through speech recognition, ' +
	'so expect misheard or misspelled words, fragments in another language and run-on ' +
	'phrasing. Infer the intended words rather than taking odd ones literally.';
const OPEN = `<cctui-session-context input="dictated">\n${NOTE}\n</cctui-session-context>\n\n`;

export function dictatedPrompt(text: string): string {
	return `${OPEN}${text}`;
}

export function splitDictated(text: string): { text: string; dictated: boolean } {
	return text.startsWith(OPEN)
		? { text: text.slice(OPEN.length), dictated: true }
		: { text, dictated: false };
}
