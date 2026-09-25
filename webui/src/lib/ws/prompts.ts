import type { ServerEvent } from '@bindings/ServerEvent';
import {
	KeyedListeners,
	type LiveAsk,
	type LivePlan,
	type PermReq,
	type SessionListPatch,
	type SoftLimitBlock,
	type ToolBlock
} from './frames';

export type PermCb = (list: PermReq[]) => void;
/** Live AskUserQuestion for a session, or null when none is pending. */
export type AskCb = (ask: LiveAsk | null) => void;
/** Live plan prompt for a session, or null when none is pending. */
export type PlanCb = (plan: LivePlan | null) => void;
/** Live soft-limit block for a session, or null when none is active. */
export type SoftLimitCb = (sl: SoftLimitBlock | null) => void;
export type ToolBlockCb = (b: ToolBlock | null) => void;

/** How prompt changes reach the session list. */
export interface PromptListHost {
	emitListPatch(p: SessionListPatch): void;
	markListDirty(): void;
}

/** Per-session prompts awaiting the user: permissions, questions, plans,
 * soft-limit and tool-call blocks. Not reactive; changes are pushed to the
 * per-session listeners. */
export class LivePrompts {
	/** pending permission prompts, keyed by session id; not reactive */
	private perms = new Map<string, PermReq[]>();
	/** pending AskUserQuestion, keyed by session id; not reactive */
	private asks = new Map<string, LiveAsk>();
	private plans = new Map<string, LivePlan>();
	private softLimits = new Map<string, SoftLimitBlock>();
	private toolBlocks = new Map<string, ToolBlock>();
	private permCbs = new KeyedListeners<PermReq[]>();
	private askCbs = new KeyedListeners<LiveAsk | null>();
	private planCbs = new KeyedListeners<LivePlan | null>();
	private softLimitCbs = new KeyedListeners<SoftLimitBlock | null>();
	private toolBlockCbs = new KeyedListeners<ToolBlock | null>();

	constructor(private host: PromptListHost) {}

	/** Apply a prompt frame. Returns false for frames this does not own. */
	handleFrame(msg: ServerEvent): boolean {
		switch (msg.type) {
			case 'permission_request': {
				const { type: _, ...p } = msg;
				const list = this.perms.get(p.session_id) ?? [];
				if (!list.some((x) => x.request_id === p.request_id)) {
					this.setPerms(p.session_id, [...list, p]);
				}
				return true;
			}
			case 'permission_resolved':
				this.removePerm(msg.session_id, msg.request_id);
				return true;
			case 'ask_question': {
				this.setAsk(msg.session_id, {
					question: msg.question,
					questions: msg.questions ?? null,
					preamble: msg.preamble ?? null
				});
				return true;
			}
			case 'ask_resolved':
				this.setAsk(msg.session_id, null);
				return true;
			case 'plan_request': {
				this.setPlan(msg.session_id, { plan: msg.plan, preamble: msg.preamble ?? null });
				return true;
			}
			case 'plan_resolved':
				this.setPlan(msg.session_id, null);
				return true;
			case 'soft_limit_reached': {
				const { type: _, session_id, ...sl } = msg;
				this.setSoftLimit(session_id, sl);
				return true;
			}
			case 'tool_call_blocked': {
				this.setToolBlock(msg.session_id, { tool_name: msg.tool_name, rule: msg.rule });
				return true;
			}
			case 'soft_limit_cleared':
				this.setSoftLimit(msg.session_id, null);
				return true;
			default:
				return false;
		}
	}

	removePerm(sid: string, requestId: string) {
		this.setPerms(
			sid,
			(this.perms.get(sid) ?? []).filter((x) => x.request_id !== requestId)
		);
	}

	// Gaining attention patches the list item in place (the client knows the
	// session just became blocked); losing it needs the server-derived bucket,
	// so that path falls back to the debounced refetch.
	private setPerms(id: string, list: PermReq[]) {
		this.perms.set(id, list);
		if (list.length > 0) {
			this.host.emitListPatch({ session_id: id, attention: 'needs_input', bucket: 'blocked' });
		} else {
			this.host.markListDirty();
		}
		this.permCbs.emit(id, list);
	}

	private setAsk(id: string, ask: LiveAsk | null) {
		if (ask === null) this.asks.delete(id);
		else this.asks.set(id, ask);
		if (ask) this.host.emitListPatch({ session_id: id, attention: 'needs_input', bucket: 'blocked' });
		else this.host.markListDirty();
		this.askCbs.emit(id, ask);
	}

	private setPlan(id: string, plan: LivePlan | null) {
		if (plan === null) this.plans.delete(id);
		else this.plans.set(id, plan);
		if (plan) this.host.emitListPatch({ session_id: id, attention: 'needs_input', bucket: 'blocked' });
		else this.host.markListDirty();
		this.planCbs.emit(id, plan);
	}

	private setSoftLimit(id: string, sl: SoftLimitBlock | null) {
		if (sl === null) this.softLimits.delete(id);
		else this.softLimits.set(id, sl);
		this.host.markListDirty();
		this.softLimitCbs.emit(id, sl);
	}

	private setToolBlock(id: string, b: ToolBlock | null) {
		if (b === null) this.toolBlocks.delete(id);
		else this.toolBlocks.set(id, b);
		this.toolBlockCbs.emit(id, b);
	}

	/** Current pending permission count for a session (read in list templates;
	 * the list re-derives on changeTick, which `setPerms` bumps). */
	pendingCount(id: string): number {
		return this.perms.get(id)?.length ?? 0;
	}

	/** Register a pending-permissions listener for a session. Fires with the
	 * current list immediately and on every change. Returns an unsubscribe fn. */
	onPerms(id: string, cb: PermCb): () => void {
		const off = this.permCbs.add(id, cb);
		cb(this.perms.get(id) ?? []);
		return off;
	}

	/** Register a live AskUserQuestion listener for a session. Fires with the
	 * current pending question (or null) immediately and on every change.
	 * Returns an unsubscribe fn. */
	onAsk(id: string, cb: AskCb): () => void {
		const off = this.askCbs.add(id, cb);
		cb(this.asks.get(id) ?? null);
		return off;
	}

	/** Clear any live pending question for a session (e.g. after the user
	 * answers, before the daemon's resolution event arrives). */
	clearAsk(id: string) {
		if (this.asks.has(id)) this.setAsk(id, null);
	}

	/** Register a live plan-prompt listener for a session. Fires with
	 * the current pending plan (or null) immediately and on every change.
	 * Returns an unsubscribe fn. */
	onPlan(id: string, cb: PlanCb): () => void {
		const off = this.planCbs.add(id, cb);
		cb(this.plans.get(id) ?? null);
		return off;
	}

	/** Clear any live pending plan for a session (e.g. after the user answers,
	 * before the daemon's resolution event arrives). */
	clearPlan(id: string) {
		if (this.plans.has(id)) this.setPlan(id, null);
	}

	/** Register a live soft-limit listener for a session. Fires with
	 * the current block (or null) immediately and on every change. Returns an
	 * unsubscribe fn. Mirrors `onAsk`/`onPlan` so the banner keeps its state in
	 * component-local `$state`, never reading a keyed `$state` off this singleton
	 * via `$derived`. */
	onSoftLimit(id: string, cb: SoftLimitCb): () => void {
		const off = this.softLimitCbs.add(id, cb);
		cb(this.softLimits.get(id) ?? null);
		return off;
	}

	/** Latest tool-call block for a session, until dismissed. Fires with the
	 * current one immediately. */
	onToolBlock(id: string, cb: ToolBlockCb): () => void {
		const off = this.toolBlockCbs.add(id, cb);
		cb(this.toolBlocks.get(id) ?? null);
		return off;
	}

	dismissToolBlock(id: string) {
		if (this.toolBlocks.has(id)) this.setToolBlock(id, null);
	}

	/** Clear any live soft-limit block for a session (e.g. immediately after the
	 * user switches accounts, before the server's `soft_limit_cleared` arrives). */
	clearSoftLimit(id: string) {
		if (this.softLimits.has(id)) this.setSoftLimit(id, null);
	}
}
