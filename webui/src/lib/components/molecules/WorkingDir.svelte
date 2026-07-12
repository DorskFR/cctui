<script lang="ts">
	import { Badge, truncate } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import { copyText } from '$lib/clipboard';

	// Fish-style working-dir chip, shared by the chat header and the session
	// card. Leads with a 📁 glyph and progressively abbreviates the *ancestor*
	// segments (one at a time, left→right) as the available width shrinks —
	// `/home/dorsk/Documents/cctui` → `/h/dorsk/Documents/cctui` → … →
	// `/h/d/D/cctui` — so the leaf (basename) stays whole as long as possible.
	// Below that the leaf itself ellipsises, never narrower than `minLeaf` chars.
	let {
		path,
		minLeaf = 18,
		mono = true,
		full = false,
		style = ''
	}: {
		path: string;
		/** Char floor the chip reserves so the (truncated) leaf is always legible. */
		minLeaf?: number;
		mono?: boolean;
		/** Show the whole path at its natural width — skip the fit/abbreviate
		 * algorithm entirely. The chip sizes to content (flex:none) so the caller
		 * can let it wrap to the next row rather than shrink. */
		full?: boolean;
		style?: string;
	} = $props();

	// Candidate renderings, richest → poorest. We pick the first that fits the
	// measured width; the last (truncated leaf) is the hard floor.
	const candidates = $derived.by(() => {
		const trimmed = path.replace(/\/+$/, '');
		const absolute = trimmed.startsWith('/');
		const segs = trimmed.split('/').filter(Boolean);
		if (segs.length === 0) return [trimmed || '/'];
		const leaf = segs[segs.length - 1];
		const parents = segs.slice(0, -1);
		const out: string[] = [];
		// Abbreviate parents from the left, one extra each step.
		for (let n = 0; n <= parents.length; n++) {
			const shown = parents.map((p, i) => (i < n ? p.charAt(0) : p));
			const joined = [...shown, leaf].join('/');
			out.push(absolute ? `/${joined}` : joined);
		}
		// Leaf alone, then progressively truncated down to the min floor.
		out.push(leaf);
		for (let keep = leaf.length - 1; keep >= minLeaf; keep--) {
			// `max` counts the ellipsis, so keep+1 yields `keep` visible chars + `…`.
			out.push(truncate(leaf, { max: keep + 1, mode: 'end' }));
		}
		return out;
	});

	let avail = $state(Infinity); // measured slot width, in px
	let chPx = $state(8); // measured width of one mono char, in px
	// Non-text chrome (badge padding + border + 📁 glyph + gap), in px. Measured
	// as badge.offsetWidth − text.offsetWidth, which is text-independent (the
	// text term cancels), so it stays correct even while the text is clipped.
	let chrome = $state(0);

	// The rail claims the flex slot and is what we measure; the colored Badge
	// sizes to its content INSIDE it (left-aligned), so the leftover space is
	// transparent — no empty box — and `avail` is independent of which candidate
	// we picked (measuring the Badge itself would collapse to the chosen text).
	let rail: HTMLSpanElement | undefined = $state();
	let txt: HTMLSpanElement | undefined = $state();
	let probe: HTMLSpanElement | undefined = $state();

	$effect(() => {
		if (!rail) return;
		const measure = () => {
			avail = rail!.clientWidth;
			if (probe) chPx = probe.getBoundingClientRect().width / 10 || chPx;
			const badgeEl = txt?.parentElement;
			if (badgeEl && txt) chrome = Math.max(0, badgeEl.offsetWidth - txt.offsetWidth);
		};
		measure();
		const ro = new ResizeObserver(measure);
		ro.observe(rail);
		return () => ro.disconnect();
	});

	const shown = $derived.by(() => {
		if (full) return path.replace(/\/+$/, '') || '/';
		// 1px safety so a sub-pixel rounding error never re-introduces a clip.
		const budget = Math.max(0, avail - chrome - 1);
		const fit = candidates.find((c) => c.length * chPx <= budget);
		return fit ?? candidates[candidates.length - 1];
	});
</script>

<!-- Transparent rail: claims the flex slot (grows to fill), measured for the
     available width. The badge inside hugs its content. -->
<span
	bind:this={rail}
	style="display:flex;align-items:center;min-width:0;{full ? 'flex:none' : 'overflow:hidden;flex:1 1 0'};{style}"
>
	<Badge
		as="button"
		{mono}
		title={m.sessions_workdir_copy_title({ path })}
		style="display:inline-flex;align-items:center;gap:0.25em;min-width:0;max-width:100%;overflow:hidden;text-align:left"
		onpointerdown={(e: PointerEvent) => e.stopPropagation()}
		onclick={(e: MouseEvent) => {
			e.stopPropagation();
			copyText(path);
		}}
	>
		<span aria-hidden="true" style="flex:none">📁</span>
		<span bind:this={txt} style="overflow:hidden;white-space:nowrap;text-overflow:clip">{shown}</span>
		<!-- Offscreen 10-char ruler: one mono char's px width for the fit math. -->
		<span
			bind:this={probe}
			aria-hidden="true"
			style="position:absolute;visibility:hidden;white-space:pre;pointer-events:none">{'0'.repeat(10)}</span
		>
	</Badge>
</span>
