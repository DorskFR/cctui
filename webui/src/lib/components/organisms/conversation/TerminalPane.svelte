<script lang="ts">
	// Read-only live terminal. Asks the server to watch the session's PTY, loads
	// xterm.js in parallel, and writes relayed bytes into the terminal. Nothing is
	// stored server-side: a fresh daemon attach repaints the current screen on
	// open, so a late viewer still sees the live frame. Never sends input
	// upstream — strictly a video feed.
	import { onMount } from 'svelte';
	import { IconButton } from '@dorsk/tsumikit';
	import { ws } from '$lib/ws.svelte';
	import { m } from '$lib/paraglide/messages';
	import { resolveTerminalFont, resolveTerminalBg, BUNDLED_TERMINAL_FONT } from './terminalFont';
	import { fitScale, PHONE_QUERY } from './terminalFit';
	import { PtyStream, loadTerminalFont } from './terminalStream.svelte';

	// Past this the pane opens with fallback-font metrics rather than staying blank.
	const FONT_LOAD_CAP_MS = 500;

	// The daemon's held/viewer attach is fixed at 120x40 (ATTACH_COLS/ROWS); size
	// the viewport to match so the geometry never fights the PTY.
	const COLS = 120;
	const ROWS = 40;

	let { sessionId, onclose }: { sessionId: string; onclose: () => void } = $props();

	let host = $state<HTMLDivElement | null>(null);
	const stream = new PtyStream();
	let fit = $state<HTMLDivElement | null>(null);
	let available = $state(0);
	let natural = $state({ width: 0, height: 0 });
	let phone = $state(false);
	// On a phone the fixed 120-column frame is scaled down to the pane width
	// rather than resized, so the daemon's attach geometry is untouched.
	const scale = $derived(phone ? fitScale(available, natural.width) : 1);
	const scaled = $derived(scale < 1);

	onMount(() => {
		const mq = window.matchMedia(PHONE_QUERY);
		const onMq = () => (phone = mq.matches);
		onMq();
		mq.addEventListener('change', onMq);
		let disposed = false;
		let term: import('@xterm/xterm').Terminal | null = null;

		// Ask before xterm/fonts are ready: the repaint that follows the daemon
		// attach is the only frame carrying the current screen. Chunks that beat
		// the terminal are buffered and flushed by `attach`.
		const offPty = ws.onPty(sessionId, stream.push);
		ws.watchPty(sessionId);

		// xterm and its CSS are browser-only; adapter-static SSRs, so load lazily.
		void (async () => {
			const [[{ Terminal }]] = await Promise.all([
				Promise.all([
					import('@xterm/xterm'),
					import('@xterm/xterm/css/xterm.css'),
					import('$lib/styles/terminal-font.css')
				]),
				// Measure glyph width only after the bundled font is loaded, else
				// xterm sizes cells from a fallback font and glyphs come out
				// spaced-out. Capped, and never `document.fonts.ready` — that waits
				// on every pending font on the page.
				loadTerminalFont(BUNDLED_TERMINAL_FONT, FONT_LOAD_CAP_MS)
			]);
			if (disposed || !host) return;

			term = new Terminal({
				cols: COLS,
				rows: ROWS,
				convertEol: false,
				disableStdin: true,
				scrollback: 1000,
				fontSize: 12,
				fontFamily: resolveTerminalFont(),
				theme: { background: resolveTerminalBg() }
			});
			term.open(host);
			if (fit) natural = { width: fit.offsetWidth, height: fit.offsetHeight };
			stream.attach(term);
		})();

		return () => {
			disposed = true;
			mq.removeEventListener('change', onMq);
			offPty();
			stream.detach();
			ws.unwatchPty(sessionId);
			term?.dispose();
		};
	});
</script>

<div class="term-pane">
	<div class="term-head">
		<span class="term-title">
			<span class="term-dot" class:on={stream.live}></span>
			{stream.live
				? m.conversation_terminal_readonly_live()
				: m.conversation_terminal_connecting()}
		</span>
		<IconButton
			inline
			glyphSize={14}
			icon="x"
			label={m.conversation_terminal_close_aria()}
			onclick={onclose}
		/>
	</div>
	<div class="term-host" bind:clientWidth={available}>
		<div class="term-sizer" class:scaled style:height={scaled ? `${natural.height * scale}px` : undefined}>
			<div
				class="term-fit"
				bind:this={fit}
				style:transform={scaled ? `scale(${scale})` : undefined}
			>
				<div bind:this={host}></div>
			</div>
		</div>
	</div>
</div>

<style>
	.term-pane {
		display: flex;
		flex-direction: column;
		border: 1px solid var(--border-strong);
		border-radius: var(--r-md);
		margin: var(--sp-2) var(--sp-3);
		overflow: hidden;
		background: var(--term-bg);
	}
	.term-head {
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: var(--sp-1) var(--sp-2);
		background: var(--bg-elevated-2);
		border-bottom: 1px solid var(--border);
		font-size: var(--fs-xs);
	}
	.term-title {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-2);
		color: var(--text-muted);
	}
	.term-dot {
		width: 8px;
		height: 8px;
		border-radius: 50%;
		background: var(--border-strong);
	}
	.term-dot.on {
		background: var(--ok);
		box-shadow: 0 0 6px var(--ok);
	}
	.term-host {
		overflow: auto;
	}
	.term-sizer.scaled {
		overflow: hidden;
	}
	.term-fit {
		display: inline-block;
		padding: var(--sp-1);
		transform-origin: top left;
	}
	@media (max-width: 959px) {
		.term-host {
			max-height: 50svh;
		}
	}
</style>
