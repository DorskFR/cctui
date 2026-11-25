/** Uppercases and hyphenates as the user types: `abcd1` → `ABCD-1`. */
export function formatTypedCode(raw: string): string {
	const s = raw.replace(/[^a-zA-Z0-9]/g, '').toUpperCase().slice(0, 8);
	return s.length > 4 ? `${s.slice(0, 4)}-${s.slice(4)}` : s;
}
