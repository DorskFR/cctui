// Browsers hide `dataTransfer` payloads until the drop, so a hovering zone
// cannot read the dragged account id; the card publishes it here on dragstart.
// Touch has no HTML5 drag at all: the handle drives the same state with
// pointer events and `overId` marks the zone under the finger.
export const accountDrag = $state({ accountId: '', overId: '' });

/** The pool zone under a viewport point, by its `data-pool-id`. */
export function poolZoneAt(x: number, y: number): string {
	return document.elementFromPoint(x, y)?.closest('[data-pool-id]')?.getAttribute('data-pool-id') ?? '';
}
