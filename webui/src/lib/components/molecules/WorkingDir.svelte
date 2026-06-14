<script lang="ts">
	import { Badge, truncate } from '@dorsk/tsumikit';
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
		style = ''
	}: {
		path: string;
		/** Char floor the chip reserves so the (truncated) leaf is always legible. */
		minLeaf?: number;
		mono?: boolean;
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

	let avail = $state(Infinity); // measured inner width, in px
	let chPx = $state(8); // measured width of one mono char, in px

	let probe: HTMLSpanElement | undefined = $state();

	$effect(() => {
		if (!probe) return;
		const measure = () => {
			const parent = probe!.parentElement;
			if (parent) avail = parent.clientWidth;
			chPx = probe!.getBoundingClientRect().width / 10 || chPx;
		};
		measure();
		const ro = new ResizeObserver(measure);
		if (probe.parentElement) ro.observe(probe.parentElement);
		return () => ro.disconnect();
	});

	// The 📁 glyph + a hair of trailing room; leave it out of the path budget.
	const reserve = $derived(chPx * 2.5);
	const shown = $derived.by(() => {
		const budget = Math.max(0, avail - reserve);
		const fit = candidates.find((c) => c.length * chPx <= budget);
		return fit ?? candidates[candidates.length - 1];
	});
</script>

<Badge
	as="button"
	{mono}
	title="Click to copy — {path}"
	style="display:inline-flex;align-items:center;gap:0.25em;min-width:0;max-width:100%;overflow:hidden;text-align:left;{style}"
	onpointerdown={(e: PointerEvent) => e.stopPropagation()}
	onclick={(e: MouseEvent) => {
		e.stopPropagation();
		copyText(path);
	}}
>
	<span aria-hidden="true" style="flex:none">📁</span>
	<span style="overflow:hidden;white-space:nowrap;text-overflow:clip">{shown}</span>
	<!-- Offscreen 10-char ruler: gives us one mono char's px width for the fit math. -->
	<span
		bind:this={probe}
		aria-hidden="true"
		style="position:absolute;visibility:hidden;white-space:pre;pointer-events:none">{'0'.repeat(10)}</span
	>
</Badge>
