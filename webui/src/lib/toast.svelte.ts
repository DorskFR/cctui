// Optional inline action on a toast (e.g. "Undo" after an archive). Clicking
// it dismisses the toast first, then runs the handler, so the action can only
// fire once and only while the toast is still on screen.
export interface ToastAction {
	label: string;
	run: () => void | Promise<void>;
}

export interface Toast {
	id: number;
	text: string;
	kind: 'ok' | 'err' | 'info';
	action?: ToastAction;
}

// Toasts carrying an action linger longer than plain ones: the action is only
// reachable while the toast is visible, so give the user time to react.
export const ACTION_TOAST_MS = 7000;

class Toasts {
	items = $state<Toast[]>([]);
	private seq = 0;

	push(text: string, kind: Toast['kind'] = 'info', ms = 3500, action?: ToastAction) {
		const id = ++this.seq;
		this.items = [...this.items, { id, text, kind, action }];
		setTimeout(() => this.dismiss(id), ms);
		return id;
	}
	ok(text: string, action?: ToastAction) {
		return this.push(text, 'ok', action ? ACTION_TOAST_MS : 3500, action);
	}
	err(text: string) {
		return this.push(text, 'err', 5000);
	}
	dismiss(id: number) {
		this.items = this.items.filter((t) => t.id !== id);
	}
	// Fire a toast's action: the toast goes away first so a double-tap can't
	// run the handler twice, then the handler runs (errors surface as a toast).
	act(id: number) {
		const t = this.items.find((x) => x.id === id);
		if (!t?.action) return;
		this.dismiss(id);
		void Promise.resolve()
			.then(t.action.run)
			.catch((e) => this.err(e instanceof Error ? e.message : String(e)));
	}
}

export const toasts = new Toasts();
