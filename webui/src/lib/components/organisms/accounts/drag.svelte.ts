// Browsers hide `dataTransfer` payloads until the drop, so a hovering zone
// cannot read the dragged account id; the card publishes it here on dragstart.
export const accountDrag = $state({ accountId: '' });
