// The transcript is windowed twice — client-side (`renderLimit`) and
// server-side (`before` paging) — so an older message may be in neither. The
// jump primitive walks both windows outward until the seq is mounted.

/** The render-window half of the primitive, owned by `Conversation.svelte`
 *  (which holds `renderLimit`) and bound out to the drawer. */
export interface RenderWindow {
	isRendered: (seq: number) => boolean;
	grow: () => void;
}

export interface SeqJumpDeps {
	/** `seq` is among the fetched events. */
	hasSeq: (seq: number) => boolean;
	/** `seq` is inside the currently rendered tail window. */
	isRendered: (seq: number) => boolean;
	/** Grow the render window by one chunk (via `holdForPrepend`). */
	growRender: () => void;
	/** More history exists beyond the fetched window. */
	canFetchOlder: () => boolean;
	fetchOlder: () => Promise<void>;
	/** Centre + flash the line; false when it is not mounted yet. */
	centerOnSeq: (seq: number) => boolean;
	unstick: () => void;
	/** Let pending DOM updates land between steps. */
	settle?: () => Promise<void>;
	/** Bounds the grow/fetch walk so a missing seq can't spin forever. */
	maxSteps?: number;
}

const nextFrame = (): Promise<void> =>
	new Promise((resolve) => {
		if (typeof requestAnimationFrame === 'undefined') setTimeout(resolve, 0);
		else requestAnimationFrame(() => resolve());
	});

export function createSeqJumper(deps: SeqJumpDeps): {
	ensureSeqVisible: (seq: number) => Promise<boolean>;
} {
	const settle = () => (deps.settle ?? nextFrame)();
	const maxSteps = deps.maxSteps ?? 60;

	async function ensureSeqVisible(seq: number): Promise<boolean> {
		if (!Number.isFinite(seq)) return false;
		deps.unstick();
		for (let step = 0; step <= maxSteps; step++) {
			if (deps.hasSeq(seq)) {
				if (deps.isRendered(seq)) {
					if (deps.centerOnSeq(seq)) return true;
					await settle();
					return deps.centerOnSeq(seq);
				}
				deps.growRender();
			} else {
				if (!deps.canFetchOlder()) return false;
				await deps.fetchOlder();
			}
			await settle();
		}
		return false;
	}

	return { ensureSeqVisible };
}
