<script lang="ts">
	import type { SessionListItem } from '@bindings/SessionListItem';
	import { statusBadgeClass, modelShort, modelFamily } from '$lib/format';
	import MachineBadge from '$lib/components/molecules/MachineBadge.svelte';
	import AccountBadge from '$lib/components/molecules/AccountBadge.svelte';
	import SessionDot from '$lib/components/molecules/SessionDot.svelte';
	import LabelBadge from '$lib/components/molecules/LabelBadge.svelte';
	import TokenUsage from '$lib/components/molecules/TokenUsage.svelte';
	import WorkingDir from '$lib/components/molecules/WorkingDir.svelte';
	import SubagentBadge from '$lib/components/molecules/SubagentBadge.svelte';
	import type { Label } from '@bindings/Label';
	import AdapterIcon from '$lib/components/atoms/AdapterIcon.svelte';
	import { Badge, Button, Card, Cluster, Stack, Text, Timestamp } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import { escapeHtml } from '$lib/markdown';
	import { highlightTerms } from '$lib/search';
	import { isStaleWorking, toolActivity, formatAgo } from '../../../routes/sessions/sessions.logic';
	import { onMount } from 'svelte';

	let {
		session,
		child = false,
		compact: dense = false,
		grid = false,
		pendingCount = 0,
		unreadCount = 0,
		onopen,
		selectable = false,
		selected = false,
		onToggleSelect,
		swipeable = false,
		swipeLabel = m.sessions_archive(),
		onSwipe,
		onTogglePin,
		highlight = [],
		subagentCost = null,
		subagentToggles = [],
		stacked = false,
		// Label editing (CCT-360): when `onAttachLabel` is supplied the card shows
		// the inline add/remove picker; otherwise the chips render read-only.
		allLabels = [],
		onCreateLabel,
		onAttachLabel,
		onDetachLabel,
		onUpdateLabel,
		onDeleteLabel,
		// Draft sessions (CCT-394): staged spawns not yet launched. When `draft`,
		// the card body click is inert (no drawer) and a Launch/Edit/Discard action
		// group renders in place of the live-session affordances, so drafts share
		// the exact same compact-list / card layout as every other section.
		draft = false,
		draftLaunching = false,
		preview = null,
		onLaunch,
		onEdit,
		onDiscard,
		accentHue = null
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
		// Unread assistant messages (CCT-580): a red count pill, distinct from the
		// amber pending-permission badge. Suppressed at 0 or for the open session
		// (the caller passes 0 there).
		unreadCount?: number;
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
		// Subagent group toggles (CCT-297 #?): collapsible (>=3 agent) groups this
		// session parents, rendered as count badges in the leading gutter slot so
		// they share the title's left edge instead of hanging in an external rail.
		subagentToggles?: {
			key: string;
			count: number;
			running: number;
			open: boolean;
			label: string;
			ontoggle: () => void;
		}[];
		// Stacked surface (CCT-297): in card view, a conversation that parents
		// subagents is drawn as a stacked card (a pile peeking out bottom-right) so
		// it reads as "has more behind it" at a glance.
		stacked?: boolean;
		// Label editing (CCT-360).
		allLabels?: Label[];
		onCreateLabel?: (name: string, color: string) => Promise<Label>;
		onAttachLabel?: (id: string, labelId: string) => void | Promise<void>;
		onDetachLabel?: (id: string, labelId: string) => void | Promise<void>;
		onUpdateLabel?: (labelId: string, patch: { name?: string; color?: string }) => Promise<Label>;
		onDeleteLabel?: (labelId: string) => void | Promise<void>;
		// Draft affordances (CCT-394).
		draft?: boolean;
		draftLaunching?: boolean;
		// Optional message-preview override (drafts show their staged prompt here,
		// since a not-yet-launched session has no last message).
		preview?: string | null;
		onLaunch?: (s: SessionListItem) => void;
		onEdit?: (s: SessionListItem) => void;
		onDiscard?: (s: SessionListItem) => void;
		// Color-by accent hue (CCT-466): a left-border strip tinting the card by its
		// label / working dir / machine. null = no accent.
		accentHue?: number | null;
	} = $props();

	const s = $derived(session);
	// Detailed card (grid, not compact) has room to spare, so its footer chips
	// keep their natural size and wrap to a second row instead of shrinking.
	const detailed = $derived(grid && !dense);
	// Match snippet with search terms wrapped in <mark> (escape first → safe HTML).
	const snippetHtml = $derived(
		s.match_snippet && highlight.length
			? highlightTerms(escapeHtml(s.match_snippet), highlight)
			: null
	);
	// Drafts pass their staged prompt as `preview` since there's no last message.
	const lastMsg = $derived(preview ?? s.last_message_text);
	const dirName = $derived(s.working_dir.split('/').filter(Boolean).pop() || '');
	// Subagents inherit the parent's working dir, so the dir-basename fallback
	// makes every child read the same ("cctui"). Give nameless subagents the
	// short id (the adjacent "subagent" badge already labels the kind), so
	// siblings are distinguishable without a redundant "subagent ·" prefix.
	const title = $derived(s.name || (child ? s.id.slice(0, 6) : dirName || s.id));
	const needsInput = $derived(s.attention === 'needs_input' && s.status !== 'archived');
	// Hibernated (CCT-228): worker exited but resumable — a reply revives it
	// (daemon resume-on-reply). Red dot, mirroring claude's own agents view.
	// Stale Working sessions (CCT-365): a derived, time-based display signal that
	// re-evaluates on a clock tick (60s — the 30-min horizon doesn't need finer)
	// and clears the instant fresh activity (a newer `last_heartbeat`, bumped by
	// subagent work too per CCT-366) arrives. Not a persisted state.
	// 5s tick (CCT-594): the tool-cadence age needs second-ish freshness to read
	// "grinding" vs "asleep"; the coarse 30-min stale signal rides the same clock.
	let now = $state(Date.now());
	onMount(() => {
		const t = setInterval(() => (now = Date.now()), 5_000);
		return () => clearInterval(t);
	});
	const stale = $derived(isStaleWorking(s, now));
	const act = $derived(toolActivity(s, now));
	const livenessClass = $derived(
		s.hibernated
			? 'dot-hibernated'
			: stale
				? 'dot-stale'
				: s.liveness === 'active'
					? 'dot-active'
					: s.liveness === 'stale'
						? 'dot-stale'
						: 'dot-dead'
	);
	const u = $derived(s.token_usage);
	// Subagent cost rollup (CCT-297 #19): only meaningful when there are agents.
	const rollup = $derived(subagentCost && subagentCost.count > 0 ? subagentCost : null);
	// Liveness is conveyed by the colored dot, so the badge only carries the
	// meaningful lifecycle states ("new", "archived"), not active/inactive.
	const showStatusBadge = $derived(s.status === 'new' || s.status === 'archived');
	// Translate the server status enum at render (never the raw value itself).
	const statusLabel = (st: string): string => {
		switch (st) {
			case 'new':
				return m.sessions_status_new();
			case 'archived':
				return m.sessions_status_archived();
			case 'active':
				return m.sessions_status_active();
			case 'inactive':
				return m.sessions_status_inactive();
			case 'dead':
				return m.sessions_status_dead();
			case 'draft':
				return m.sessions_status_draft();
			default:
				return st;
		}
	};
	// Label picker is only interactive on top-level rows with an attach handler.
	const labelEditable = $derived(!!onAttachLabel && !child);

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

	// Surface state is passed inline on the Card element: the swipe
	// transform/transition, the compact tighter padding, and the attention/selection
	// tints. (Scoped CSS can't reach a child component's root element, so these can't
	// live in the style block; they're card-instance state, not reusable rules, so
	// inline is the right home.)
	const cardStyle = $derived(
		[
			// Only apply the transform while actually swiped: a `transform` creates a
			// stacking context, which would trap the stacked-card pseudo-elements
			// (z-index:-1/-2) inside the card and paint them OVER its background
			// instead of behind it (CCT-297). At rest (swipeX 0) we omit it so the
			// stack peeks out behind as intended.
			swipeX !== 0 ? `transform: translateX(${swipeX}px)` : '',
			`transition: ${swiping ? 'none' : 'transform 0.2s var(--ease)'}`,
			// Opaque attention fill (not the translucent --attention-bg): a parent with
			// subagents renders as a `stacked` Card, and a see-through front surface lets
			// the back-stack pseudo-elements (z-index:-1/-2) bleed through the card body
			// instead of only peeking at the edges (CCT-349).
			needsInput ? 'background: var(--attention-bg-solid); border-left: 3px solid var(--attention-bar)' : '',
			// Subagent (child) cards carry an info-tinted border so they read as part
			// of the parent's stacked group (matches the "subagent" info badge).
			child && !needsInput ? 'border-color: color-mix(in srgb, var(--info) 45%, var(--border))' : '',
			// Color-by accent (CCT-466, CCT-651): the dimension's hue tints the whole
			// card so types read at a distance. The left strip resolves against the
			// theme's --mach-border-sl pair (same infra as MachineBadge); the body tint
			// mixes a sliver of the pure hue into --bg-elevated so lightness/contrast
			// track each theme automatically. needsInput keeps its own opaque attention
			// fill + bar and stays dominant over the tint.
			accentHue != null && !needsInput
				? `--mh:${accentHue}; background: color-mix(in srgb, hsl(var(--mh) 65% 50%) 8%, var(--bg-elevated)); border-left: 3px solid hsl(var(--mh) var(--mach-border-sl))`
				: '',
			selected
				? 'background: color-mix(in srgb, var(--accent) 12%, var(--bg-elevated)); box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--accent) 55%, transparent)'
				: ''
		]
			.filter(Boolean)
			.join('; ')
	);

	function handleClick(e?: MouseEvent) {
		// Clicks on a nested overlay control (e.g. the Timestamp details popover)
		// bubble up to the card; they shouldn't also open the session.
		if (e?.target instanceof Element && e.target.closest('[popovertarget],[popover]')) return;
		// Drafts aren't openable — their action buttons handle everything.
		if (draft) return;
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
	class:stale
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
				<span>{swipeProgress >= 1 ? m.sessions_swipe_release({ action: swipeLabel.toLowerCase() }) : swipeLabel}</span>
			</div>
		</div>
	{/if}
	<!-- Tappable surface is the tsumikit Card as a <div> (NOT a <button>): the row
	     hosts its own interactive controls (label color pickers, the +-popover,
	     remove buttons), which can't legally nest inside a <button>. `tap` carries
	     the hover/active affordance; role/tabindex/onkeydown restore button a11y.
	     The layout is built ENTIRELY from kit primitives (Stack / Cluster) — no card
	     class overrides; the only per-card styling is the surface state passed inline
	     on the Card (scoped CSS can't reach a child component's root element). -->
	<Card
		as="div"
		tap
		stacked={stacked}
		stackTone="info"
		padding={dense && !grid ? 'sm' : 'md'}
		role="button"
		tabindex={0}
		style={cardStyle}
		onclick={handleClick}
		onkeydown={(e: KeyboardEvent) => {
			if (e.key === 'Enter' || e.key === ' ') {
				e.preventDefault();
				handleClick();
			}
		}}
	>
		{#if dense && !grid}
			<!-- COMPACT LIST = ONE real row: a single no-wrap Cluster whose direct
			     children are the surviving fields, left→right. No nested bands. -->
			<Cluster wrap={false} gap="var(--sp-2)">
				{@render gutter()}
				{@render engine()}
				{@render titleText()}
				<!-- Message takes the slack and ellipsises aggressively. -->
				{#if s.match_snippet || lastMsg}
					<Text
						truncate
						tone={s.match_snippet ? 'default' : 'muted'}
						size="xs"
						style="flex:1 1 0;min-width:0"
						>{s.match_snippet ? `🔍 ${s.match_snippet}` : lastMsg}</Text
					>
				{:else}
					<span style="flex:1 1 auto"></span>
				{/if}
				<!-- No working-dir chip in compact list: the basename already leads the
				     title, and a bare folder glyph here can't be hovered or copied. -->
				{@render activity()}
				{@render unreadBadge()}
				{@render time()}
				{#if s.model}<Text tone="muted" size="xs" style="flex:none;white-space:nowrap">{modelFamily(s.model)}</Text>{/if}
				{#if draft}{@render draftActions()}{:else}{@render logo()}{/if}
			</Cluster>
		{:else}
			<!-- DETAILED / PROJ = stacked bands (Stack), each horizontal band a Cluster. -->
			<Stack gap="var(--sp-2)" style={grid ? 'height:100%' : ''}>
				<!-- 1. LEAD: gutter · dot · engine · title · labels ···· status · perm · time
				     Labels live on this first row, right after the title. The lead group
				     flex-wraps, so when the title + chips can't fit the row width the chips
				     drop to a second line WITHOUT dragging the status/perm/time group with
				     them — that `.trail` group is pinned top-right (align="flex-start" on
				     the Cluster, which it applies via a style: directive so a style="" on
				     it would be silently overridden). Both groups carry the same min-height
				     so on a single line everything reads vertically centered, while a wrapped
				     label line sits cleanly below (row-gap) instead of overlapping. -->
				<Cluster wrap={false} gap="var(--sp-2)" align="flex-start">
					<span class="lead">
						{@render gutter()}
						{@render engine()}
						{@render titleText()}
						{#if s.labels.length > 0 || labelEditable}
							<LabelBadge
								labels={s.labels}
								editable={labelEditable}
								{allLabels}
								onCreate={onCreateLabel}
								onAttach={(lid) => onAttachLabel?.(s.id, lid)}
								onDetach={(lid) => onDetachLabel?.(s.id, lid)}
								onUpdate={onUpdateLabel}
								onDelete={onDeleteLabel}
							/>
						{/if}
						{@render activity()}
					</span>
					<span class="trail">
						{#if showStatusBadge}<Badge class={statusBadgeClass(s.status)} style="padding:0.05rem var(--sp-2)">{statusLabel(s.status)}</Badge>{/if}
						{@render unreadBadge()}
						{#if pendingCount > 0}<Badge tone="warn" style="padding:0.05rem var(--sp-2)">{m.sessions_perm_count({ count: pendingCount })}</Badge>{/if}
						{#if s.auto_approve}<Badge tone="warn" style="padding:0.05rem var(--sp-2)" title={m.sessions_auto_approve_title()}>⚡</Badge>{/if}
						{@render time()}
					</span>
				</Cluster>

				<!-- 2. PREVIEW: multi-line clamp (grid grows to fill). -->
				{#if s.match_snippet}
					<div class="preview match" style={grid ? 'flex:1 1 auto' : ''}>🔍 {#if snippetHtml}{@html snippetHtml}{:else}{s.match_snippet}{/if}</div>
				{:else if lastMsg}
					<div class="preview last muted" style={grid ? 'flex:1 1 auto' : ''}>{lastMsg}</div>
				{/if}

				<!-- 3. FOOTER: path ···· tokens · Σ · model · logo. Wraps when tight so a
				     long model can't shove the logo out (grid pins it to the bottom). -->
				<Cluster gap="var(--sp-2)" style={grid ? 'margin-top:auto' : ''}>
					<!-- Fish-style working-dir chip: leaf stays whole, ancestors abbreviate
					     as width shrinks (see WorkingDir). In detailed cards it keeps its
					     natural width (no shrink) and the footer wraps; elsewhere it flexes. -->
					<WorkingDir path={s.working_dir} full={detailed} style={detailed ? '' : 'max-width:22rem'} />
					<Cluster wrap={false} gap="var(--sp-2)" style="margin-left:auto;flex:none">
						{#if draft}
							{#if s.model}<Text tone="muted" size="xs" style="flex:none;white-space:nowrap">{modelShort(s.model)}{s.effort ? ` · ${s.effort}` : ''}</Text>{/if}
							{@render draftActions()}
						{:else}
							<TokenUsage usage={u} cold={s.cache_cold} sum={rollup ? rollup.tokens : null} />
							{#if s.model}<Text tone="muted" size="xs" truncate={!detailed} style={detailed ? 'flex:none;white-space:nowrap' : 'max-width:14rem;flex:none'}>{modelShort(s.model)}{s.effort ? ` · ${s.effort}` : ''}</Text>{/if}
							{@render logo()}
						{/if}
					</Cluster>
				</Cluster>
			</Stack>
		{/if}
	</Card>
</div>

<!-- ── Shared field snippets (rendered into both the compact row and the detailed
     bands so the markup isn't duplicated) ───────────────────────────────────── -->
{#snippet gutter()}
	{#if selectable}
		<span class="gutter check" class:on={selected} aria-hidden="true">{selected ? '✓' : ''}</span>
	{:else}
		<!-- Toggle badges, ↳ indent, and star all share the one fixed gutter slot so
		     titles line up. A parent can carry both a toggle and a star, so they sit
		     side by side here rather than one excluding the other. -->
		<span class="gutter-group">
			{#each subagentToggles as t (t.key)}
				<SubagentBadge
					count={t.count}
					running={t.running}
					open={t.open}
					label={t.label}
					ontoggle={t.ontoggle}
				/>
			{/each}
			{#if child}
				<span class="gutter indent" title={m.sessions_subagent_badge()} aria-hidden="true">↳</span>
			{:else if onTogglePin}
				<span
					class="gutter star"
					class:on={s.pinned}
					role="button"
					tabindex="0"
					title={s.pinned ? m.sessions_unpin_title() : m.sessions_pin_title()}
					aria-pressed={s.pinned}
					aria-label={s.pinned ? m.sessions_unpin_aria() : m.sessions_pin_aria()}
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
		</span>
	{/if}
{/snippet}

{#snippet engine()}
	<SessionDot session={s} {livenessClass} {now} />
	{#if stale}
		<Badge tone="warn" style="padding:0.05rem var(--sp-2)" title={m.sessions_stale_title()}>{m.sessions_stale_badge()}</Badge>
	{/if}
	{#if child}
		<Badge tone="info" style="padding:0.05rem var(--sp-2)">{m.sessions_subagent_badge()}</Badge>
	{:else}
		<MachineBadge name={s.machine_name} id={s.machine_id} hue={s.machine_hue} mono />
		<AccountBadge name={s.account_name} />
	{/if}
{/snippet}

{#snippet titleText()}
	<!-- Compact row: the title shows in full up to a max-width cap (~28ch, but never
	     more than ~55% of the row) and ellipsises past it; the message preview gets
	     `flex:1 1 0` so it grows into whatever space is left WITHOUT exerting shrink
	     pressure on the title. A short title shrink-wraps to its content (no blank
	     gap), a long title caps and the message fills the remainder. (A flat
	     `min-width` floor was wrong both ways: it reserved space for short titles —
	     blank gap — and `fit-content(18ch)` is an invalid min-width value so it was
	     dropped, leaving the truncate class's min-width:0 → squished to nothing.)
	     Detailed/grid bands keep the plain shrink-to-fit. -->
	<Text
		weight="semibold"
		size={dense ? 'md' : 'lg'}
		truncate
		style={dense && !grid ? 'flex:0 1 auto;min-width:0;max-width:min(28ch,55%)' : 'flex:0 1 auto;min-width:0'}
		>{title}</Text
	>
{/snippet}

{#snippet time()}
	{#if s.last_message_at}<span style="flex:none;white-space:nowrap"
			><Timestamp value={s.last_message_at} mode="relative" tone="faint" size="xs" /></span
		>{/if}
{/snippet}

{#snippet logo()}
	<span style="flex:none;display:inline-flex"><AdapterIcon adapter={s.adapter_id} size={14} /></span>
{/snippet}

{#snippet unreadBadge()}
	{#if unreadCount > 0}<Badge
			tone="danger"
			active
			size="sm"
			style="flex:none"
			title={m.sessions_unread_title({ count: unreadCount })}>{unreadCount}</Badge
		>{/if}
{/snippet}

<!-- Live tool cadence (CCT-594): a dense "⚙N · Xs" chip that distinguishes a
     grinding session (fresh tool calls, incl. subagent roll-ups) from one that
     looks alive but is asleep (no tool call for minutes → amber). The detail
     headline rides alongside in the roomier detailed/grid bands only. -->
{#snippet activity()}
	{#if act.show && !stale}
		<span
			class="activity"
			class:asleep={act.asleep}
			title={act.detail ??
				(act.asleep ? m.sessions_activity_asleep_title() : m.sessions_activity_live_title())}
		>
			<span class="act-cadence"
				>⚙{act.count}{#if act.ageMs !== null}&nbsp;·&nbsp;{formatAgo(act.ageMs)}{/if}</span
			>
			{#if act.detail && !dense}<span class="act-detail">{act.detail}</span>{/if}
		</span>
	{/if}
{/snippet}

<!-- Draft action group (CCT-394): Launch / Edit / Discard. Each stops propagation
     so a button tap never bubbles to the card surface. Rendered in the trailing
     slot of both the compact row and the detailed/grid footer. -->
{#snippet draftActions()}
	<span
		class="draft-actions"
		role="presentation"
		onpointerdown={(e) => e.stopPropagation()}
		onclick={(e) => e.stopPropagation()}
	>
		<Button size="sm" variant="primary" disabled={draftLaunching} onclick={() => onLaunch?.(s)}>
			{#if draftLaunching}<span class="spin"></span>{/if}
			{m.sessions_launch()}
		</Button>
		<Button size="sm" onclick={() => onEdit?.(s)}>{m.common_edit()}</Button>
		<Button size="sm" variant="danger" onclick={() => onDiscard?.(s)}>{m.sessions_discard()}</Button>
	</span>
{/snippet}

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
	/* Stale Working session (CCT-365): dim the whole card so a long-idle session
	   reads as "needs attention, not live" at a glance. Paired with the amber
	   dot + "stale" badge. Re-evaluated on the clock tick; clears on activity. */
	.sc-wrap.stale {
		opacity: 0.6;
	}
	/* Grid cards (CCT-305): the wrapper and the card fill the grid cell's full
	   height so every card in a row matches (the grid stretches the cells), and
	   the footer pins to the bottom for a uniform silhouette regardless of how
	   much middle content each card has. */
	.sc-wrap.grid {
		height: 100%;
	}
	/* Lead group of the detailed/grid header band: gutter · engine · title · labels.
	   Wraps so the label chips fall to a second line when the title is long / the
	   row is narrow, while the trailing status/perm/time group (`.trail`) stays
	   pinned to the first line. Both share `min-height` (one lg-title line) so a
	   single-line row reads vertically centered; `row-gap` keeps a wrapped label
	   line off the machine badge above it. */
	.lead {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		column-gap: var(--sp-2);
		row-gap: var(--sp-2);
		flex: 1 1 auto;
		min-width: 0;
		min-height: 1.75rem;
	}
	.trail {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		flex: none;
		min-height: 1.75rem;
	}
	/* ── Last-message preview (detailed / grid only) ────────────────────────────
	   The compact row renders the message as a <Text truncate> inline; here it's a
	   multi-line clamp. List clamps to 3 lines; grid grows (flex set inline) and
	   clamps to 6. (This is a native element WE render, so plain scoped CSS works.) */
	.preview {
		min-width: 0;
		overflow: hidden;
		font-size: var(--fs-sm);
		white-space: normal;
		display: -webkit-box;
		-webkit-line-clamp: 3;
		line-clamp: 3;
		-webkit-box-orient: vertical;
	}
	/* Detailed card view is the spacious one (CCT-345): give the message preview
	   far more verticality so the card reads tall, not wide. */
	.sc-wrap.grid:not(.dense) .preview {
		min-height: 0;
		-webkit-line-clamp: 12;
		line-clamp: 12;
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
	/* Compact mode is a flat list — no indent for subagents. */
	.sc-wrap.dense.child {
		margin-left: 0;
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
	/* Draft action group (CCT-394): keeps Launch/Edit/Discard on one line, sharing
	   the trailing slot of both the compact row and the detailed footer. */
	.draft-actions {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-1);
		flex: none;
	}
	/* Fixed gutter slot: star / checkbox / ↳ all share it so titles align. */
	.gutter {
		flex: none;
	}
	/* Holds the toggle badge(s) + star/↳ together in the single gutter slot so a
	   parent that's both collapsible and pinnable shows both, side by side. */
	.gutter-group {
		flex: none;
		display: inline-flex;
		align-items: center;
		gap: var(--sp-1);
	}
	.gutter.indent {
		color: var(--text-faint);
		font-size: var(--fs-md);
		line-height: 1;
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
	/* Live tool-cadence chip (CCT-594). Dense, muted, single line; the cadence
	   count/age stays whole while the optional detail headline ellipsises. Amber
	   when asleep — the evidence-based "looks alive but wedged" tell. */
	.activity {
		display: inline-flex;
		align-items: baseline;
		gap: var(--sp-1);
		min-width: 0;
		flex: 0 1 auto;
		font-size: var(--fs-xs);
		color: var(--text-faint);
		white-space: nowrap;
	}
	.act-cadence {
		flex: none;
		font-variant-numeric: tabular-nums;
	}
	.act-detail {
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		color: var(--text-muted);
		max-width: 22rem;
	}
	.activity.asleep {
		color: var(--warn);
	}
	.activity.asleep .act-detail {
		color: var(--warn);
	}
	/* Search match snippet (CCT-184): accent rule + clamp, sharing the .preview
	   sizing above so the snippet sits in the same slot as the message preview. */
	.match {
		color: var(--text);
		border-left: 2px solid var(--accent, #88c0d0);
		padding-left: var(--sp-2);
	}
</style>
