<script lang="ts">
	import type { SessionListItem } from '@bindings/SessionListItem';
	import { relativeTime, statusBadgeClass, timestampTooltip } from '$lib/format';
	import MachineBadge from '$lib/components/molecules/MachineBadge.svelte';
	import LabelBadge from '$lib/components/molecules/LabelBadge.svelte';
	import TokenUsage from '$lib/components/molecules/TokenUsage.svelte';
	import type { Label } from '@bindings/Label';
	import AdapterIcon from '$lib/components/atoms/AdapterIcon.svelte';
	import { Badge, Card, Text } from '@dorsk/tsumikit';
	import { escapeHtml } from '$lib/markdown';
	import { highlightTerms } from '$lib/search';
	import { copyText } from '$lib/clipboard';

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
		subagentCost = null,
		// Label editing (CCT-360): when `onAttachLabel` is supplied the card shows
		// the inline add/remove picker; otherwise the chips render read-only.
		allLabels = [],
		onCreateLabel,
		onAttachLabel,
		onDetachLabel
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
		// Label editing (CCT-360).
		allLabels?: Label[];
		onCreateLabel?: (name: string, color: string) => Promise<Label>;
		onAttachLabel?: (id: string, labelId: string) => void | Promise<void>;
		onDetachLabel?: (id: string, labelId: string) => void | Promise<void>;
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
	<!-- Tappable surface is the tsumikit Card as a <div> (NOT a <button>): the row
	     hosts its own interactive controls (label color pickers, the +-popover,
	     remove buttons), which can't legally nest inside a <button>. `tap` carries
	     the hover/active affordance; role/tabindex/onkeydown restore button a11y. -->
	<Card
		as="div"
		tap
		role="button"
		tabindex={0}
		class={[
			'sc stack',
			child && 'child',
			dense && 'dense',
			grid && 'grid',
			needsInput && 'attn',
			selectable && 'selectable',
			selected && 'selected'
		]
			.filter(Boolean)
			.join(' ')}
		style="transform: translateX({swipeX}px)"
		onclick={handleClick}
		onkeydown={(e: KeyboardEvent) => {
			if (e.key === 'Enter' || e.key === ' ') {
				e.preventDefault();
				handleClick();
			}
		}}
	>
	<!-- Canonical field model (CCT-320/321): the SAME skeleton renders in all four
	     variants (list/card × compact/detailed). Density only changes spacing, the
	     preview line count, and grid sizing — never WHERE a field lives.
	       1. Lead row: star · status dot · machine badge · title · [hand/perm/status] · spacer · subagent Σ chip
	       2. Preview:  last-message / search snippet (compact = 1 line + …, detailed = multi-line)
	       3. Meta row: model name + logo · token sum (ONCE) · timestamp · path -->
	<!-- 1. LEAD ── [gutter] · dot · machine/subagent · title  ····  status · perm · time
	     lead-left grows (pushing lead-right to the edge); lead-right is the upper
	     half of the right scan-column (the lower half is the footer's logo). -->
	<div class="row lead">
		<div class="lead-left">
			<!-- Gutter: one fixed slot — checkbox (select mode) / ↳ (child) / star. -->
			{#if selectable}
				<span class="gutter check" class:on={selected} aria-hidden="true">{selected ? '✓' : ''}</span>
			{:else if child}
				<span class="gutter indent" title="subagent" aria-hidden="true">↳</span>
			{:else if onTogglePin}
				<span
					class="gutter star"
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
			<span class="dot {livenessClass}"></span>
			{#if child}
				<Badge tone="info" style="padding:0.05rem var(--sp-2)">subagent</Badge>
			{:else}
				<MachineBadge name={s.machine_name} id={s.machine_id} hue={s.machine_hue} mono />
			{/if}
			<Text class="title" weight="semibold" truncate>{title}</Text>
			<!-- Labels + add (CCT-360): right after the title. Read-only chips
			     everywhere; the inline add/remove picker only on top-level rows with an
			     attach handler. The popover is portaled out of this card. -->
			{#if s.labels.length > 0 || (onAttachLabel && !child)}
				<LabelBadge
					labels={s.labels}
					editable={!!onAttachLabel && !child}
					{allLabels}
					onCreate={onCreateLabel}
					onAttach={(lid) => onAttachLabel?.(s.id, lid)}
					onDetach={(lid) => onDetachLabel?.(s.id, lid)}
				/>
			{/if}
		</div>
		<div class="lead-right">
			{#if showStatusBadge}<Badge class={statusBadgeClass(s.status)} style="padding:0.05rem var(--sp-2)">{s.status}</Badge>{/if}
			{#if pendingCount > 0}<Badge tone="warn" style="padding:0.05rem var(--sp-2)">{pendingCount} perm</Badge>{/if}
			{#if s.last_message_at}<Text
					class="ago"
					tone="faint"
					size="xs"
					title={timestampTooltip(s.registered_at, s.last_message_at, s.last_activity_at)}
					>{relativeTime(s.last_message_at)}</Text
				>{/if}
		</div>
	</div>

	<!-- 3. PREVIEW ── last-message / search snippet. Compact = single ellipsised
	     line; detailed/grid = multi-line clamp. `🔍` marks the search variant. -->
	{#if s.match_snippet}
		<div class="preview match">🔍 {#if snippetHtml}{@html snippetHtml}{:else}{s.match_snippet}{/if}</div>
	{:else if s.last_message_text}
		<div class="preview last muted">{s.last_message_text}</div>
	{/if}

	<!-- 4. FOOTER ── path  ····  tokens · Σ · model · logo. The right cluster is the
	     lower half of the right scan-column; the logo is the rightmost anchor and the
	     ONLY footer field kept in compact (path/tokens/Σ/model drop). Path flexes and
	     truncates from the LEFT so the working-dir basename stays visible. -->
	<div class="row meta">
		<!-- Path is a copy-to-clipboard chip (stopPropagation so it never opens the
		     drawer). Truncates from the LEFT so the basename stays visible. -->
		<Badge
			as="button"
			mono
			class="path"
			title="Click to copy — {s.working_dir}"
			onpointerdown={(e: PointerEvent) => e.stopPropagation()}
			onclick={(e: MouseEvent) => {
				e.stopPropagation();
				copyText(s.working_dir);
			}}>{s.working_dir}</Badge
		>
		<div class="meta-right">
			<!-- Σ leads the token block; when this parent spawned subagents we pass the
			     parent+subagents aggregate as the sum so Σ reflects the true cost. -->
			<span class="tok"
				><TokenUsage usage={u} cold={s.cache_cold} sum={rollup ? rollup.tokens : null} /></span
			>
			{#if s.model}<Text class="model" tone="muted" size="xs">{s.model}{s.effort ? ` · ${s.effort}` : ''}</Text>{/if}
			<span class="logo"><AdapterIcon adapter={s.adapter_id} size={14} /></span>
		</div>
	</div>
	</Card>
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
	/* ── Canonical preview clamp (CCT-320/321) ──────────────────────────────
	   The last-message preview is the only field whose SIZE differs by variant;
	   its POSITION (between lead row and meta row) is constant. Compact = single
	   truncated line with a plain `…`; detailed = a multi-line clamp. No fade. */
	.preview {
		min-width: 0;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		font-size: var(--fs-sm);
	}
	.sc-wrap.dense .preview {
		font-size: var(--fs-xs);
	}
	/* Detailed (list + card): multi-line clamp. List clamps to 3 lines; grid lets
	   the preview grow to fill the card's free vertical space (still clamped). */
	.sc-wrap:not(.dense) .preview {
		white-space: normal;
		display: -webkit-box;
		-webkit-line-clamp: 3;
		line-clamp: 3;
		-webkit-box-orient: vertical;
	}
	/* Card grid: the card fills its cell; the preview flex-grows into the gap
	   between the lead row and the pinned meta row so tall cards aren't empty. */
	.sc.grid {
		height: 100%;
		min-height: 9rem;
	}
	.sc-wrap.grid .meta {
		margin-top: auto;
	}
	.sc.grid:not(.dense) {
		/* Detailed cards are the MOST spacious view (CCT-321): taller floor + more
		   padding than compact cards, and the preview grows to fill. */
		min-height: 13rem;
		padding: var(--sp-4);
	}
	.sc-wrap.grid:not(.dense) .preview {
		flex: 1 1 auto;
		min-height: 0;
		-webkit-line-clamp: 6;
		line-clamp: 6;
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
		/* Subagent rows: just a plain 1px border, no faded/clipped left-border
		   accent (CCT-345). The indent already signals the parent relationship. */
		border: 1px solid var(--border);
		background: var(--bg-elevated);
		color: var(--text);
	}
	.sc.dense {
		padding: var(--sp-2) var(--sp-3);
		gap: var(--sp-1);
	}
	/* Compact mode is a flat list — no indent for subagents. */
	.sc-wrap.dense.child {
		margin-left: 0;
	}
	.sc.dense.child {
		border: 1px solid var(--border);
	}
	/* ── Compact list = one row per session ───────────────────────────────────
	   Detailed stacks four bands (lead / labels / preview / footer). Compact
	   FLATTENS them onto a single line by subtraction: `display:contents` dissolves
	   the .lead and .meta band wrappers so their leaf groups (lead-left, lead-right,
	   meta-right) become direct flex children of .sc alongside .preview, then `order`
	   welds them into the canonical left→right run:
	       lead-left · preview(flex) · lead-right(status·perm·time) · logo
	   The inboard footer fields (path · tokens · Σ · model) and the label strip drop
	   out — nothing re-orders, so every surviving field keeps its detailed position.
	   Scope is list-compact only: grid-compact stays a stacked card. */
	.sc.dense:not(.grid) {
		flex-direction: row;
		align-items: center;
		flex-wrap: nowrap;
		gap: var(--sp-2);
	}
	.sc-wrap.dense:not(.grid) .lead,
	.sc-wrap.dense:not(.grid) .meta {
		display: contents;
	}
	.sc-wrap.dense:not(.grid) .lead-left {
		order: 1;
		flex: 0 1 auto;
		min-width: 0;
	}
	.sc-wrap.dense:not(.grid) .preview {
		order: 2;
		flex: 1 1 auto;
	}
	.sc-wrap.dense:not(.grid) .lead-right {
		order: 3;
		flex: none;
	}
	.sc-wrap.dense:not(.grid) .meta-right {
		order: 4;
		flex: none;
	}
	/* Subtracted in compact: path and the inboard cost fields — only the logo (the
	   rightmost engine anchor) survives in .meta-right. */
	.sc-wrap.dense:not(.grid) :global(.path),
	.sc-wrap.dense:not(.grid) .meta-right .tok,
	.sc-wrap.dense:not(.grid) .meta-right :global(.model) {
		display: none;
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
	/* Lead row: [gutter · dot · machine/subagent · title] grows and pushes the
	   right cluster [status · perm · time] to the edge. Nowrap — the title is the
	   only thing that shrinks (min-width:0), so the right cluster never wraps below. */
	.lead {
		gap: var(--sp-2);
		flex-wrap: nowrap;
		align-items: center;
	}
	.lead-left {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		flex: 1 1 auto;
		min-width: 0;
	}
	.lead-right {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		flex: none;
	}
	/* Fixed gutter slot: star / checkbox / ↳ all share it so titles align. */
	.gutter {
		flex: none;
	}
	.gutter.indent {
		color: var(--text-faint);
		font-size: var(--fs-md);
		line-height: 1;
	}
	.sc :global(.title) {
		font-size: var(--fs-md);
		/* Don't grow (CCT-360): size to content so inline labels + the add button
		   sit right after the title; the trailing spacer absorbs free space. Still
		   shrinks (min-width:0) to truncate long titles when the row is tight. */
		flex: 0 1 auto;
		min-width: 0;
	}
	/* Detailed views give the title more presence (CCT-321: clearly visible title). */
	.sc:not(.dense) :global(.title) {
		font-size: var(--fs-lg, 1.05rem);
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
	/* Footer row: path (grows, left) ···· [tokens · Σ · model · logo] (right cluster,
	   the lower half of the right scan-column). Nowrap — the path is the only growable
	   element and truncates first; the right cluster is content-sized and protected. */
	.meta {
		gap: var(--sp-2);
		flex-wrap: nowrap;
		align-items: center;
	}
	.meta-right {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		flex: none;
		margin-left: auto;
	}
	.tok {
		display: inline-flex;
		flex: none;
	}
	/* Provider logo: the rightmost anchor, present in every variant. */
	.logo {
		display: inline-flex;
		flex: none;
	}
	/* Model label: keep it from eating the row when names are long. */
	.meta-right :global(.model) {
		flex: none;
		max-width: 14rem;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	@media (max-width: 639px) {
		.meta-right :global(.model) {
			max-width: 8rem;
		}
	}
	/* Relative-time hint ("4m ago") must never wrap to a second line (CCT-297 #15). */
	.lead-right :global(.ago) {
		flex: none;
		white-space: nowrap;
	}
	/* Working-dir path chip: sized to its content and pinned LEFT — it does NOT
	   stretch to the token cluster (which is pushed right by .meta-right's
	   margin-auto). Capped by max-width and truncated from the LEFT (`direction:rtl`)
	   so the basename — the part that identifies the session — stays visible. */
	.meta :global(.path) {
		flex: 0 1 auto;
		max-width: 22rem;
		min-width: 0;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		direction: rtl;
		text-align: left;
	}
	/* Search match snippet (CCT-184): accent rule + clamp, sharing the .preview
	   sizing above so the snippet sits in the same slot as the message preview. */
	.match {
		color: var(--text);
		border-left: 2px solid var(--accent, #88c0d0);
		padding-left: var(--sp-2);
	}
</style>
