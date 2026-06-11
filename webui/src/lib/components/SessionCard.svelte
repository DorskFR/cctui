<script lang="ts">
	import type { SessionListItem } from '@bindings/SessionListItem';
	import { relativeTime, uptime, statusBadgeClass, timestampTooltip, compact } from '$lib/format';
	import MachineBadge from './MachineBadge.svelte';
	import TokenUsage from './TokenUsage.svelte';
	import AdapterIcon from './AdapterIcon.svelte';
	import { escapeHtml } from '$lib/markdown';
	import { highlightTerms } from '$lib/search';

	let {
		session,
		child = false,
		compact: dense = false,
		grid = false,
		pendingCount = 0,
		onopen,
		selectable = false,
		selected = false,
		onToggleSelect,
		swipeable = false,
		swipeLabel = 'Archive',
		onSwipe,
		onTogglePin,
		highlight = [],
		subagentCost = null
	}: {
		session: SessionListItem;
		child?: boolean;
		compact?: boolean;
		// Grid (card-view) layout (CCT-305): keeps cards uniform — single-line cwd
		// path (ellipsis, no wrap) and full-height fill — so a row of cards is the
		// same height with no ragged wrapping. List view leaves this false so the
		// detailed card keeps wrapping the full path (seeing it whole is the point).
		grid?: boolean;
		pendingCount?: number;
		onopen: (s: SessionListItem) => void;
		// Search terms to highlight in the match snippet (CCT-187).
		highlight?: string[];
		// Multi-select mode (CCT-172): when `selectable`, a tap toggles selection
		// instead of opening the drawer, and a checkbox is shown.
		selectable?: boolean;
		selected?: boolean;
		onToggleSelect?: (s: SessionListItem) => void;
		// Swipe-to-archive (CCT-172): on touch, a left-swipe of the row past a
		// threshold fires `onSwipe` (archive, or unarchive in the archived view) —
		// the same gesture as archiving an email. Disabled in multi-select mode so
		// it never fights checkbox tapping.
		swipeable?: boolean;
		swipeLabel?: string;
		onSwipe?: (s: SessionListItem) => void;
		// Pin/star toggle (CCT-267): when provided, a star button appears in the
		// header. Pinned sessions sort to the top and skip auto-archive.
		onTogglePin?: (s: SessionListItem) => void;
		// Rolled-up subagent usage (CCT-297 #19): on a parent that spawned
		// subagents, the parent's own tokens plus the aggregated tokens of all its
		// subagents, with the subagent count. Reported in tokens (CCT-301 #2).
		subagentCost?: { tokens: number; count: number } | null;
	} = $props();

	const s = $derived(session);
	// Match snippet with search terms wrapped in <mark> (escape first → safe HTML).
	const snippetHtml = $derived(
		s.match_snippet && highlight.length
			? highlightTerms(escapeHtml(s.match_snippet), highlight)
			: null
	);
	const dirName = $derived(s.working_dir.split('/').filter(Boolean).pop() || '');
	// Subagents inherit the parent's working dir, so the dir-basename fallback
	// makes every child read the same ("cctui"). Give nameless subagents the
	// short id (the adjacent "subagent" badge already labels the kind), so
	// siblings are distinguishable without a redundant "subagent ·" prefix.
	const title = $derived(s.name || (child ? s.id.slice(0, 6) : dirName || s.id));
	const needsInput = $derived(s.attention === 'needs_input' && s.status !== 'archived');
	// Hibernated (CCT-228): worker exited but resumable — a reply revives it
	// (daemon resume-on-reply). Red dot, mirroring claude's own agents view.
	const livenessClass = $derived(
		s.hibernated
			? 'dot-hibernated'
			: s.liveness === 'active'
				? 'dot-active'
				: s.liveness === 'stale'
					? 'dot-stale'
					: 'dot-dead'
	);
	const u = $derived(s.token_usage);
	// Subagent cost rollup (CCT-297 #19): only meaningful when there are agents.
	const rollup = $derived(subagentCost && subagentCost.count > 0 ? subagentCost : null);
	// CCT-233: the active/inactive status badges are pure noise — liveness is
	// already conveyed by the colored dot. Only keep the badge for the meaningful
	// lifecycle states ("new", "archived").
	const showStatusBadge = $derived(s.status === 'new' || s.status === 'archived');

	// ── Swipe-to-archive (CCT-172, touch only) ──────────────────────────────
	// Track a dominantly-horizontal left-swipe of the row; commit (archive) once
	// it passes ~40% of the row width, otherwise spring back. Vertical scrolling
	// is preserved by only "arming" once the gesture is clearly horizontal, and
	// the browser keeps handling pan-y on the wrapper.
	let swipeX = $state(0); // current horizontal offset (≤ 0; left only)
	let swiping = $state(false); // armed → tracking a horizontal swipe
	let swipeArmed = false; // committed to horizontal (vs vertical scroll)
	let didSwipe = false; // a swipe happened → suppress the trailing click
	let sx = 0;
	let sy = 0;
	let cardW = $state(0); // row width captured at gesture start
	const swipeThreshold = $derived(cardW ? cardW * 0.4 : Infinity);
	const swipeProgress = $derived(Math.min(1, -swipeX / swipeThreshold));

	function handleClick() {
		// A drag (even one that sprang back) shouldn't also open the session.
		if (didSwipe) {
			didSwipe = false;
			return;
		}
		if (selectable) onToggleSelect?.(s);
		else onopen(s);
	}

	function swipeStart(e: PointerEvent) {
		if (!swipeable || selectable || e.pointerType !== 'touch') return;
		sx = e.clientX;
		sy = e.clientY;
		cardW = (e.currentTarget as HTMLElement).offsetWidth;
		swipeArmed = false;
		didSwipe = false;
	}
	function swipeMove(e: PointerEvent) {
		if (!swipeable || selectable || e.pointerType !== 'touch' || !cardW) return;
		const dx = e.clientX - sx;
		const dy = e.clientY - sy;
		if (!swipeArmed) {
			if (Math.abs(dx) < 12) return; // below the deadzone — undecided
			// Dominantly vertical → it's a scroll, bail out of swipe handling.
			if (Math.abs(dx) <= Math.abs(dy) * 1.5) {
				cardW = 0;
				return;
			}
			swipeArmed = true;
			swiping = true;
			try {
				(e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
			} catch {
				/* capture unsupported — move events still arrive */
			}
		}
		swipeX = Math.min(0, dx); // left only
	}
	function swipeEnd() {
		if (!swiping) {
			cardW = 0;
			swipeArmed = false;
			return;
		}
		const commit = -swipeX >= swipeThreshold;
		swiping = false;
		swipeArmed = false;
		didSwipe = true;
		if (commit) {
			if (typeof navigator !== 'undefined' && navigator.vibrate) navigator.vibrate(20);
			swipeX = -cardW; // slide the rest of the way out, then archive
			onSwipe?.(s);
		} else {
			swipeX = 0; // spring back
		}
		cardW = 0;
	}
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
	class="sc-wrap"
	class:child
	class:dense
	class:grid
	class:swiping
	onpointerdown={swipeStart}
	onpointermove={swipeMove}
	onpointerup={swipeEnd}
	onpointercancel={swipeEnd}
>
	<!-- Swipe-to-archive reveal (CCT-172): a colored layer behind the row that
	     shows as the card slides left under a touch swipe. -->
	{#if swipeable && swipeX < 0}
		<div class="swipe-reveal" style="opacity: {0.25 + 0.75 * swipeProgress}" aria-hidden="true">
			<div class="swipe-reveal-inner" class:armed={swipeProgress >= 1}>
				<svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
					<rect x="3" y="4" width="18" height="4" rx="1" />
					<path d="M5 8v11a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1V8" />
					<path d="M9 12h6" />
				</svg>
				<span>{swipeProgress >= 1 ? `Release to ${swipeLabel.toLowerCase()}` : swipeLabel}</span>
			</div>
		</div>
	{/if}
	<button
		class="card card-tap sc stack"
		class:child
		class:dense
		class:grid
		class:attn={needsInput}
		class:selectable
		class:selected
		style="transform: translateX({swipeX}px)"
		onclick={handleClick}
	>
	<div class="row top">
		{#if selectable}
			<span class="check" class:on={selected} aria-hidden="true">{selected ? '✓' : ''}</span>
		{/if}
		{#if child}<span class="sub" title="subagent">↳</span>{/if}
		{#if dense && !child}<MachineBadge name={s.machine_name} id={s.machine_id} hue={s.machine_hue} mono />{/if}
		<span class="dot {livenessClass}"></span>
		<span class="title truncate">{title}</span>
		{#if onTogglePin && !child}
			<span
				class="star"
				class:on={s.pinned}
				role="button"
				tabindex="0"
				title={s.pinned ? 'Unpin' : 'Pin to top (exempt from auto-archive)'}
				aria-pressed={s.pinned}
				aria-label={s.pinned ? 'Unpin session' : 'Pin session'}
				onpointerdown={(e) => e.stopPropagation()}
				onclick={(e) => {
					e.stopPropagation();
					onTogglePin?.(s);
				}}
				onkeydown={(e) => {
					if (e.key === 'Enter' || e.key === ' ') {
						e.preventDefault();
						e.stopPropagation();
						onTogglePin?.(s);
					}
				}}>{s.pinned ? '★' : '☆'}</span
			>
		{/if}
		{#if child}<span class="badge badge-info tag">subagent</span>{/if}
		{#if needsInput}<span class="hand" title="needs input">✋</span>{/if}
		{#if pendingCount > 0}<span class="badge badge-warn">{pendingCount} perm</span>{/if}
		{#if dense && (s.match_snippet || s.last_message_text)}
			<span class="preview muted"
				>{#if snippetHtml}{@html snippetHtml}{:else}{s.match_snippet ??
						s.last_message_text}{/if}</span
			>
		{:else}
			<div class="spacer"></div>
		{/if}
		{#if dense}
			{#if showStatusBadge}<span class="badge {statusBadgeClass(s.status)} tag">{s.status}</span>{/if}
			{#if s.last_message_at}<span
					class="faint sm ago"
					title={timestampTooltip(s.registered_at, s.last_message_at, s.last_activity_at)}
					>{relativeTime(s.last_message_at)}</span
				>{/if}
		{/if}
		{#if s.model}<span class="muted sm model">{s.model}{s.effort ? ` · ${s.effort}` : ''}</span>{/if}
		{#if rollup}<span
				class="rollup sm"
				title="Total tokens incl. {rollup.count} subagent{rollup.count === 1 ? '' : 's'}"
				>Σ {compact(rollup.tokens)} tok</span
			>{/if}
		<AdapterIcon adapter={s.adapter_id} size={16} />
	</div>

	{#if !dense}
		<div class="row meta row-wrap">
			<MachineBadge name={s.machine_name} id={s.machine_id} hue={s.machine_hue} />
			{#if showStatusBadge}<span class="badge {statusBadgeClass(s.status)}">{s.status}</span>{/if}
			<span class="muted sm">up {uptime(Number(s.uptime_secs))}</span>
		</div>

		<div class="row dir" title={s.working_dir}>
			<span class="faint sm">📁</span>
			<span class="path sm">{s.working_dir}</span>
		</div>

		{#if s.match_snippet}
			<div class="match">🔍 {#if snippetHtml}{@html snippetHtml}{:else}{s.match_snippet}{/if}</div>
		{:else if s.last_message_text}
			<div class="last truncate muted">{s.last_message_text}</div>
		{/if}

		<div class="row foot">
			<TokenUsage usage={u} cold={s.cache_cold} />
			{#if rollup}<span
					class="rollup sm"
					title="Parent + {rollup.count} subagent{rollup.count === 1 ? '' : 's'} aggregated tokens"
					>Σ {compact(rollup.tokens)} tok · {rollup.count} agent{rollup.count === 1 ? '' : 's'}</span
				>{/if}
			<div class="spacer"></div>
			{#if s.last_message_at}<span
					class="faint sm ago"
					title={timestampTooltip(s.registered_at, s.last_message_at, s.last_activity_at)}
					>{relativeTime(s.last_message_at)}</span
				>{/if}
		</div>
	{/if}
	</button>
</div>

<style>
	/* Swipe wrapper (CCT-172): positioning context for the reveal layer behind
	   the row, and the owner of the subagent indent (moved off the card so the
	   reveal aligns with the card edge). pan-y keeps vertical scrolling native
	   while we handle the horizontal swipe ourselves. */
	.sc-wrap {
		position: relative;
		width: 100%;
		touch-action: pan-y;
	}
	.sc-wrap.child {
		width: auto;
		margin-left: var(--sp-4);
	}
	/* Grid cards (CCT-305): the wrapper and the card fill the grid cell's full
	   height so every card in a row matches (the grid stretches the cells), and
	   the footer pins to the bottom for a uniform silhouette regardless of how
	   much middle content each card has. */
	.sc-wrap.grid {
		height: 100%;
	}
	.sc.grid {
		height: 100%;
		/* Vertical-shaped cards (CCT-305 follow-up): a min-height so each card is
		   taller than it is wide-ish, giving the 3-line last-message preview room
		   to breathe even when a card has little other content. */
		min-height: 11rem;
	}
	.sc.grid .foot {
		margin-top: auto;
	}
	/* Card view: let the latest message FILL the card's free vertical space, then
	   clip at the bottom (CCT-312). The old fixed 3-line clamp left tall cards
	   mostly empty — a short preview cut at ~3 lines with a wall of blank space
	   down to the pinned footer. Instead `.last` flex-grows into the gap between
	   the dir row and the footer; a bottom fade mask softens the cut edge when the
	   text actually fills (a short message leaves the lower region empty, so the
	   mask fades nothing visible). */
	.sc.grid .last {
		flex: 1 1 auto;
		min-height: 0;
		display: block;
		white-space: normal;
		overflow: hidden;
		-webkit-mask-image: linear-gradient(180deg, #000 calc(100% - 1.4em), transparent);
		mask-image: linear-gradient(180deg, #000 calc(100% - 1.4em), transparent);
	}
	/* Uniform single-line cwd path in grid view — never wrap to a second line
	   (the source of ragged, uneven-height cards); ellipsis instead. */
	.sc.grid .path {
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		word-break: normal;
		min-width: 0;
	}
	.sc.grid .dir {
		min-width: 0;
	}
	/* Track the finger with no transition while swiping; ease back (spring) or
	   off-screen (commit) when released. */
	.sc-wrap .sc {
		transition: transform 0.2s var(--ease);
	}
	.sc-wrap.swiping .sc {
		transition: none;
		user-select: none;
	}
	/* Colored layer revealed behind the row as it slides left. */
	.swipe-reveal {
		position: absolute;
		inset: 0;
		z-index: 0;
		display: flex;
		align-items: center;
		justify-content: flex-end;
		padding-right: var(--sp-4);
		border-radius: var(--r-md);
		background: color-mix(in srgb, var(--warn) 22%, var(--bg));
		color: var(--warn);
	}
	.swipe-reveal-inner {
		display: flex;
		align-items: center;
		gap: var(--sp-1);
		font-size: var(--fs-sm);
		font-weight: var(--fw-semibold);
		transition: transform 0.12s var(--ease);
	}
	.swipe-reveal-inner.armed {
		transform: scale(1.12);
	}
	.sc {
		gap: var(--sp-2);
		text-align: left;
		width: 100%;
		position: relative;
		z-index: 1;
	}
	.sc.child {
		border-left: 2px solid var(--border-strong);
		/* slightly recessed/faded vs a top-level card — theme-token based */
		background: color-mix(in srgb, var(--bg) 55%, var(--bg-elevated));
		color: var(--text-muted);
	}
	.sc.dense {
		padding: var(--sp-2) var(--sp-3);
		gap: 0;
	}
	/* Compact mode is a flat list — no indent for subagents. */
	.sc-wrap.dense.child {
		margin-left: 0;
	}
	.sc.dense.child {
		border-left: none;
	}
	.tag {
		font-size: var(--fs-xs);
		padding: 0.05rem var(--sp-2);
	}
	.sc.attn {
		background: var(--attention-bg);
		border-left: 3px solid var(--attention-bar);
	}
	/* Multi-select (CCT-172) */
	.sc.selected {
		background: color-mix(in srgb, var(--accent) 12%, var(--bg-elevated));
		border-color: color-mix(in srgb, var(--accent) 55%, transparent);
	}
	.check {
		flex: none;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 1.15rem;
		height: 1.15rem;
		border-radius: var(--r-sm);
		border: 1.5px solid var(--border-strong);
		background: var(--bg);
		color: var(--bg);
		font-size: 0.8rem;
		line-height: 1;
	}
	.check.on {
		background: var(--accent);
		border-color: var(--accent);
		color: var(--bg);
	}
	.top {
		gap: var(--sp-2);
	}
	.sub {
		color: var(--text-faint);
	}
	.title {
		font-weight: var(--fw-semibold);
		font-size: var(--fs-md);
	}
	.star {
		background: none;
		border: none;
		cursor: pointer;
		user-select: none;
		padding: 0;
		line-height: 1;
		font-size: var(--fs-md);
		color: var(--text-faint);
		flex: none;
	}
	.star.on {
		color: var(--warn, #e0a800);
	}
	.star:hover {
		color: var(--warn, #e0a800);
	}
	.hand {
		font-size: var(--fs-sm);
	}
	.meta {
		gap: var(--sp-2);
	}
	/* CCT-308 item 1: the footer (token usage + rollup + relative time) is a
	   no-wrap row that could run past a narrow viewport; let it wrap so the
	   detailed list row never overflows horizontally. */
	.foot {
		flex-wrap: wrap;
		row-gap: var(--sp-1);
	}
	.sm {
		font-size: var(--fs-xs);
	}
	.last {
		font-size: var(--fs-sm);
	}
	/* Model label sits just before the adapter logo in the top row; keep it from
	   eating the row when names are long. */
	.model {
		flex: none;
		max-width: 14rem;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	/* CCT-308 item 1: on narrow viewports a 14rem model label + title + badges in
	   the no-wrap top row ran wider than the screen, and the page clips horizontal
	   overflow → the right edge of detailed list rows was unreachable. Cap the
	   model label hard and let the top row wrap so nothing is pushed off-screen. */
	@media (max-width: 639px) {
		.top {
			flex-wrap: wrap;
		}
		.model {
			max-width: 8rem;
		}
	}
	/* Aggregated subagent cost chip (CCT-297 #19). */
	.rollup {
		flex: none;
		white-space: nowrap;
		font-weight: var(--fw-semibold);
		color: var(--accent, #88c0d0);
	}
	/* Relative-time hint ("4m ago") must never wrap to a second line (CCT-297 #15). */
	.ago {
		flex: none;
		white-space: nowrap;
	}
	/* Full working dir (detailed view) — break long paths rather than truncating,
	   since seeing the whole path is the point. */
	.dir {
		gap: var(--sp-1);
		align-items: baseline;
	}
	.path {
		color: var(--text-muted);
		word-break: break-all;
	}
	/* Search match snippet (CCT-184): full transcript context around the hit,
	   allowed to wrap a couple of lines so the keyword is readable. */
	.match {
		font-size: var(--fs-sm);
		color: var(--text);
		border-left: 2px solid var(--accent, #88c0d0);
		padding-left: var(--sp-2);
		display: -webkit-box;
		-webkit-line-clamp: 3;
		line-clamp: 3;
		-webkit-box-orient: vertical;
		overflow: hidden;
	}
	/* Dense-mode last-message preview: fills the space between the title and the
	   status badge, single line, ellipsis — never wraps or grows the row. */
	.preview {
		flex: 1;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		font-size: var(--fs-xs);
	}
</style>
