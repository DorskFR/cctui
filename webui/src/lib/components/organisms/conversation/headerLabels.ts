import type { Label } from '@bindings/Label';

export type LabelHandlers = {
	allLabels?: Label[];
	onCreateLabel?: (name: string, color: string) => Promise<Label>;
	/** Supplied → the strip is an editable picker; omitted → read-only. */
	onAttachLabel?: (id: string, labelId: string) => void | Promise<void>;
	onDetachLabel?: (id: string, labelId: string) => void | Promise<void>;
	onUpdateLabel?: (labelId: string, patch: { name?: string; color?: string }) => Promise<Label>;
	onDeleteLabel?: (labelId: string) => void | Promise<void>;
};
