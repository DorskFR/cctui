import type { DispatchRequest } from '@bindings/DispatchRequest';
import { NO_ACCOUNT, contextPackEnv, isCompatibleProvider, poolName } from './options';
import type { Form } from './types';

/** The dispatch request for a k8s worker. `payload` is opaque server-side and
 *  unpacked by the dispatcher into TASK_* env; empty fields are omitted so the
 *  worker's own defaults apply. `sessionId` doubles as the idempotency key. */
export function buildDispatchBody(
	form: Form,
	env: Record<string, string>,
	provider: string | undefined,
	sessionId: string
): DispatchRequest {
	const payload: Record<string, unknown> = {};
	if (form.name.trim()) payload.name = form.name.trim();
	if (form.identity.trim()) payload.identity = form.identity.trim();
	if (form.repo.trim()) payload.repo = form.repo.trim();
	if (form.ticket.trim()) payload.context = { issue_id: form.ticket.trim() };
	if (form.prompt.trim()) payload.prompt = form.prompt.trim();
	if (form.prompt_file.trim()) payload.prompt_file = form.prompt_file.trim();
	const adapter = form.dispatch_adapter || 'claude-code';
	if (adapter === 'codex') payload.adapter = 'codex';
	const compatible = !!provider && isCompatibleProvider(provider);
	const model = compatible
		? form.model_account.trim()
		: adapter === 'codex'
			? form.model_codex.trim()
			: form.model_claude.trim();
	if (model) payload.model = model;
	const effort = adapter === 'codex' ? form.effort_codex.trim() : form.effort_claude.trim();
	if (effort) payload.effort = effort;
	const fullEnv = { ...env, ...contextPackEnv(form) };
	if (Object.keys(fullEnv).length) payload.env = fullEnv;
	const timeout = form.timeout.trim() ? Number(form.timeout.trim()) : null;
	return {
		dispatcher: form.dispatcher,
		session_id: sessionId,
		timeout: Number.isFinite(timeout) ? timeout : null,
		reply_url: null,
		notify_url: null,
		notify_secret: null,
		account:
			form.account === NO_ACCOUNT || poolName(form.account) ? null : form.account.trim() || null,
		provider: provider || null,
		accounts: [],
		payload: payload as DispatchRequest['payload']
	};
}
