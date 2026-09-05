import { toasts as kit, type Toast, type ToastAction, type ToastOptions } from '@dorsk/tsumikit';

type LegacyKind = 'info' | 'ok' | 'error';

/** Kit toaster plus the pre-0.7.316 `err` / `push` names, kept until every caller has moved. */
export const toasts = Object.assign(kit, {
	err(text: string, action?: ToastAction): number {
		return kit.error(text, undefined, action);
	},
	push(text: string, kind: LegacyKind = 'info', ms = 3500, action?: ToastAction): number {
		return kit.show(text, { tone: kind, duration: ms, action });
	}
});
export type { Toast, ToastAction, ToastOptions };
