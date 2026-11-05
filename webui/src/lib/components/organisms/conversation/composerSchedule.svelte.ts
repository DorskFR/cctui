import { formatTimestamp } from '@dorsk/tsumikit';
import { errMessage } from '$lib/api';
import { m } from '$lib/paraglide/messages';
import type { PromptAttachments } from '$lib/promptAttachments.svelte';
import { pendingScheduled, useScheduledActions, useScheduledMessages } from '$lib/queries';
import { toasts } from '$lib/toast.svelte';
import { scheduleBody } from './scheduleBody';
import { parseCustom, toLocalInput } from './scheduleTimes';

export interface ComposerScheduleDeps {
	sessionId: () => string;
	draft: () => string;
	archived: () => boolean;
	att: PromptAttachments;
	stageFiles: (files: File[]) => Promise<{ paths: string[] }>;
	restore: (text: string) => void;
	sent: (text: string) => void;
}

export class ComposerSchedule {
	customOpen = $state(false);
	customValue = $state('');
	readonly #deps: ComposerScheduleDeps;
	readonly #scheduled: ReturnType<typeof useScheduledMessages>;
	readonly #actions: ReturnType<typeof useScheduledActions>;

	constructor(deps: ComposerScheduleDeps) {
		this.#deps = deps;
		this.#scheduled = useScheduledMessages(deps.sessionId);
		this.#actions = useScheduledActions(deps.sessionId);
	}

	get count(): number {
		return pendingScheduled(this.#scheduled.data).length;
	}

	get canSchedule(): boolean {
		const { att } = this.#deps;
		return (
			(!!this.#deps.draft().trim() || att.files.length > 0) &&
			!att.uploading &&
			att.images.pending.length === 0
		);
	}

	openCustom(): void {
		this.customValue = toLocalInput(new Date(Date.now() + 3_600_000));
		this.customOpen = true;
	}

	async at(at: Date): Promise<void> {
		const { att } = this.#deps;
		const text = this.#deps.draft().trim();
		if ((!text && !att.files.length) || this.#deps.archived() || att.uploading) return;
		if (att.error) {
			toasts.error(att.error);
			return;
		}
		const out = await scheduleBody(
			text,
			(t) => att.stage(t, this.#deps.stageFiles),
			(body) => this.#actions.schedule(body, at)
		);
		if (!out.ok) {
			if (out.restore !== null) this.#deps.restore(out.restore);
			if (out.error) toasts.error(m.composer_schedule_failed({ message: errMessage(out.error) }));
			return;
		}
		toasts.info(m.composer_schedule_toast({ when: formatTimestamp(at, 'datetime') }));
		this.#deps.sent(text);
	}

	confirmCustom(): void {
		const at = parseCustom(this.customValue, new Date());
		if (!at) {
			toasts.error(m.composer_schedule_custom_invalid());
			return;
		}
		this.customOpen = false;
		void this.at(at);
	}
}
