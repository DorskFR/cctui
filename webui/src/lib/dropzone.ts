// A reusable drag-and-drop file dropzone Svelte action (CCT-236).
//
//   use:dropzone={{ onFiles: (files) => …, onActive: (active) => … }}
//
// Handles the full dragenter/dragover/dragleave/drop dance with a depth counter
// (dragleave fires when the cursor crosses into a child element, so a naive
// enter/leave toggle flickers — count enters vs leaves instead). Only reacts to
// drags that actually carry files (ignores text/element drags). `onActive` is
// invoked with `true` while a file drag is hovering the node and `false` when it
// leaves or drops, so callers can render a dropzone overlay.

export interface DropzoneOpts {
	onFiles: (files: File[]) => void;
	onActive?: (active: boolean) => void;
	/** When true the dropzone ignores drags entirely (e.g. session offline). */
	disabled?: boolean;
}

function hasFiles(e: DragEvent): boolean {
	return Array.from(e.dataTransfer?.types ?? []).includes('Files');
}

export function dropzone(node: HTMLElement, opts: DropzoneOpts) {
	let current = opts;
	let depth = 0;

	const setActive = (active: boolean) => current.onActive?.(active);

	const reset = () => {
		if (depth !== 0) {
			depth = 0;
			setActive(false);
		}
	};

	const onEnter = (e: DragEvent) => {
		if (current.disabled || !hasFiles(e)) return;
		e.preventDefault();
		depth += 1;
		if (depth === 1) setActive(true);
	};

	const onOver = (e: DragEvent) => {
		if (current.disabled || !hasFiles(e)) return;
		// Must preventDefault on dragover too or the browser won't fire `drop`.
		e.preventDefault();
		if (e.dataTransfer) e.dataTransfer.dropEffect = 'copy';
	};

	const onLeave = (e: DragEvent) => {
		if (current.disabled || !hasFiles(e)) return;
		depth = Math.max(0, depth - 1);
		if (depth === 0) setActive(false);
	};

	const onDrop = (e: DragEvent) => {
		if (current.disabled || !hasFiles(e)) return;
		e.preventDefault();
		reset();
		const files = Array.from(e.dataTransfer?.files ?? []);
		if (files.length) current.onFiles(files);
	};

	node.addEventListener('dragenter', onEnter);
	node.addEventListener('dragover', onOver);
	node.addEventListener('dragleave', onLeave);
	node.addEventListener('drop', onDrop);

	return {
		update(next: DropzoneOpts) {
			current = next;
			if (next.disabled) reset();
		},
		destroy() {
			node.removeEventListener('dragenter', onEnter);
			node.removeEventListener('dragover', onOver);
			node.removeEventListener('dragleave', onLeave);
			node.removeEventListener('drop', onDrop);
		},
	};
}
