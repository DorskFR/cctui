/** macOS screenshots differ only in their trailing timestamp, so the tail
 * gets the larger share of the budget. */
export function shortFileName(name: string, max = 28): string {
	const chars = [...name];
	if (chars.length <= max) return name;
	const budget = max - 1;
	if (budget < 2) return name;
	const head = Math.floor(budget * 0.4);
	return `${chars.slice(0, head).join('')}…${chars.slice(chars.length - (budget - head)).join('')}`;
}
