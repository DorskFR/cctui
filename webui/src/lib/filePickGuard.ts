export const PICK_KEY = 'cctui_file_pick';
const FRESH_MS = 10 * 60_000;
const SETTLE_MS = 3_000;

export interface PickGuardDeps {
	store?: Pick<Storage, 'getItem' | 'setItem' | 'removeItem'>;
	now?: () => number;
}

const isFileInput = (t: EventTarget | null): t is HTMLInputElement =>
	t instanceof HTMLInputElement && t.type === 'file';

/** Phones may discard the tab while the camera app is in front, losing the
 *  picked file: a marker surviving that reload reports it via `onlost`. */
export function filePickGuard(onlost: () => void, deps: PickGuardDeps = {}) {
	return (node: HTMLElement) => {
		const store = deps.store ?? sessionStorage;
		const now = deps.now ?? Date.now;
		const clear = () => store.removeItem(PICK_KEY);

		const at = Number(store.getItem(PICK_KEY));
		if (at) {
			clear();
			if (now() - at < FRESH_MS) onlost();
		}

		let settle: ReturnType<typeof setTimeout> | undefined;
		const onclick = (e: Event) => {
			if (isFileInput(e.target)) store.setItem(PICK_KEY, String(now()));
		};
		const ondone = (e: Event) => {
			if (isFileInput(e.target)) clear();
		};
		// Without the `cancel` event, a page that turns visible again was not discarded.
		const onvisible = () => {
			if (document.visibilityState !== 'visible') return;
			clearTimeout(settle);
			settle = setTimeout(clear, SETTLE_MS);
		};
		node.addEventListener('click', onclick, true);
		node.addEventListener('change', ondone, true);
		node.addEventListener('cancel', ondone, true);
		document.addEventListener('visibilitychange', onvisible);
		return () => {
			clearTimeout(settle);
			node.removeEventListener('click', onclick, true);
			node.removeEventListener('change', ondone, true);
			node.removeEventListener('cancel', ondone, true);
			document.removeEventListener('visibilitychange', onvisible);
		};
	};
}
