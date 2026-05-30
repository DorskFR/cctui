export interface Toast {
	id: number;
	text: string;
	kind: 'ok' | 'err' | 'info';
}

class Toasts {
	items = $state<Toast[]>([]);
	private seq = 0;

	push(text: string, kind: Toast['kind'] = 'info', ms = 3500) {
		const id = ++this.seq;
		this.items = [...this.items, { id, text, kind }];
		setTimeout(() => this.dismiss(id), ms);
	}
	ok(text: string) {
		this.push(text, 'ok');
	}
	err(text: string) {
		this.push(text, 'err', 5000);
	}
	dismiss(id: number) {
		this.items = this.items.filter((t) => t.id !== id);
	}
}

export const toasts = new Toasts();
