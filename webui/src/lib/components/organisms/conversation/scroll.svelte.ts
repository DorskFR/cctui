// Sticky-bottom scroll controller for the conversation drawer. It owns the
// scroll viewport + composer textarea refs and the pin/unstick logic; the
// viewport and composer components share one instance so the textarea's
// growth can re-pin the viewport.
//
// If the user is at the bottom, new content auto-scrolls (sticky). If scrolled
// up, we don't yank them down — the viewport shows a "jump to bottom" pill.

const STICK_SLOP = 48; // px from bottom still counts as "at bottom"

export class ScrollController {
	// Bound by the viewport / composer via bind:this.
	scroller = $state<HTMLElement | undefined>(undefined);
	textarea = $state<HTMLTextAreaElement | null>(null);
	// Currently pinned to the bottom.
	stuck = $state(true);

	// The scroller's clientHeight at the time of the last settled scroll. A shrink
	// (the composer/textarea grew and stole vertical space) is a LAYOUT-induced
	// scroll event, never a user gesture, so it must not clear `stuck`. A genuine
	// user scroll-up arrives with an unchanged clientHeight.
	#lastClientHeight = 0;
	// Timestamp of the last genuine user scroll gesture (wheel / touchmove /
	// keyboard). Only such a gesture may UNSTICK the view; every other scroll
	// event is layout-induced (composer growth, new content, on-screen keyboard,
	// visualViewport resize) and must never clear the pin.
	#lastUserScroll = 0;

	markUserScroll = () => {
		this.#lastUserScroll = performance.now();
	};

	#atBottom(): boolean {
		const el = this.scroller;
		if (!el) return true;
		return el.scrollHeight - el.scrollTop - el.clientHeight <= STICK_SLOP;
	}

	onScroll = () => {
		const el = this.scroller;
		if (!el) return;
		const ch = el.clientHeight;
		const userDriven = performance.now() - this.#lastUserScroll < 200;
		// Viewport shrank since the last settled scroll (composer grew / keyboard
		// opened): layout-induced — re-stick and bail, never recompute `stuck`.
		if (ch < this.#lastClientHeight) {
			this.#lastClientHeight = ch;
			if (this.stuck) el.scrollTop = el.scrollHeight;
			return;
		}
		this.#lastClientHeight = ch;
		if (userDriven) {
			// A real gesture: honour where the user landed (may unstick).
			this.stuck = this.#atBottom();
		} else if (this.#atBottom()) {
			// Layout settled back at the bottom — re-pin, but a non-gesture scroll
			// can NEVER unstick, so a transient mid-grow position can't flip it.
			this.stuck = true;
		}
	};

	// Pin to the bottom synchronously, then again after the browser has applied
	// the reflow (rAF). Called from the composer/viewport ResizeObserver while
	// stuck. onScroll itself rejects any scroll event that coincides with a
	// clientHeight shrink, so the pin holds for arbitrarily tall composers.
	#pinAndGuard = () => {
		if (!this.stuck || !this.scroller) return;
		this.scroller.scrollTop = this.scroller.scrollHeight;
		this.#lastClientHeight = this.scroller.clientHeight;
		requestAnimationFrame(() => {
			if (!this.scroller) return;
			this.scroller.scrollTop = this.scroller.scrollHeight;
			this.#lastClientHeight = this.scroller.clientHeight;
		});
	};

	jumpToBottom = () => {
		if (this.scroller) {
			this.scroller.scrollTop = this.scroller.scrollHeight;
			this.#lastClientHeight = this.scroller.clientHeight;
		}
		this.stuck = true;
	};

	// Re-pin to the bottom (sticky). Used on send so the optimistic echo follows
	// down even if the user had scrolled up.
	stickToBottom = () => {
		this.stuck = true;
	};

	// Reset to bottom + sticky when switching sessions.
	resetForSession = () => {
		this.stuck = true;
		this.#lastClientHeight = this.scroller?.clientHeight ?? 0;
	};

	// Follow new content only when pinned to the bottom. Called from a content
	// $effect (lines/perms/working changes).
	followIfStuck = () => {
		if (this.stuck && this.scroller) {
			requestAnimationFrame(() => {
				if (this.scroller) this.scroller.scrollTop = this.scroller.scrollHeight;
			});
		}
	};

	// Hold scroll position when prepending older content (lazy render): capture
	// distance from the bottom, run `grow`, then restore so the viewport doesn't
	// jump.
	holdForPrepend = (grow: () => void) => {
		const el = this.scroller;
		const fromBottom = el ? el.scrollHeight - el.scrollTop : 0;
		grow();
		if (el)
			requestAnimationFrame(() => {
				el.scrollTop = el.scrollHeight - fromBottom;
			});
	};

	// Keep pinned to the bottom while the composer grows. Observe BOTH
	// the textarea (it grows) and the scroll viewport (its height shrinks as a
	// result) — re-pinning on the viewport's own resize is what actually keeps the
	// latest line visible. Returns a cleanup; re-run from an $effect that reads
	// `scroller` + `textarea` so it re-binds when either attaches.
	observeResize = (): (() => void) => {
		if (typeof ResizeObserver === 'undefined') return () => {};
		if (this.scroller) this.#lastClientHeight = this.scroller.clientHeight;
		const ro = new ResizeObserver(() => this.#pinAndGuard());
		if (this.textarea) ro.observe(this.textarea);
		if (this.scroller) ro.observe(this.scroller);
		return () => ro.disconnect();
	};
}
