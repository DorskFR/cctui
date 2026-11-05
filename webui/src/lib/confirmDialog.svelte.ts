import { m } from '$lib/paraglide/messages';

export interface ConfirmOptions {
	title: string;
	/** What stops working or is lost. */
	message?: string;
	/** A verb naming the act: "Delete bookmark", "Remove provider". */
	confirmLabel: string;
	cancelLabel?: string;
	tone?: 'danger' | 'warn' | 'accent';
}

export interface ConfirmRequest {
	id: number;
	title: string;
	message?: string;
	confirmLabel: string;
	cancelLabel: string;
	tone: 'danger' | 'warn' | 'accent';
}

interface Pending extends ConfirmRequest {
	resolve: (ok: boolean) => void;
}

let nextId = 1;
let queue = $state.raw<Pending[]>([]);

/** Asks the user through the one mounted `ConfirmHost`; resolves `true` on confirm,
 *  `false` on cancel or dismiss. Requests queue and show one at a time. */
export function confirmDialog(opts: ConfirmOptions): Promise<boolean> {
	return new Promise((resolve) => {
		queue = [
			...queue,
			{
				id: nextId++,
				title: opts.title,
				message: opts.message,
				confirmLabel: opts.confirmLabel,
				cancelLabel: opts.cancelLabel ?? m.common_cancel(),
				tone: opts.tone ?? 'danger',
				resolve
			}
		];
	});
}

export const confirmQueue = {
	get current(): ConfirmRequest | null {
		return queue[0] ?? null;
	},
	get size(): number {
		return queue.length;
	},
	settle(id: number, ok: boolean): void {
		const hit = queue.find((r) => r.id === id);
		if (!hit) return;
		queue = queue.filter((r) => r.id !== id);
		hit.resolve(ok);
	},
	/** Cancels everything pending (host teardown, tests). */
	clear(): void {
		const pending = queue;
		queue = [];
		for (const r of pending) r.resolve(false);
	}
};
