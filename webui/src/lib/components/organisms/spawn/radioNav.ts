/** `null` means the key does not navigate, so the caller must leave it unhandled. */
export function nextRadioIndex(key: string, current: number, total: number): number | null {
	if (total < 1 || current < 0 || current >= total) return null;
	switch (key) {
		case 'ArrowUp':
		case 'ArrowLeft':
			return (current - 1 + total) % total;
		case 'ArrowDown':
		case 'ArrowRight':
			return (current + 1) % total;
		case 'Home':
			return 0;
		case 'End':
			return total - 1;
		default:
			return null;
	}
}
