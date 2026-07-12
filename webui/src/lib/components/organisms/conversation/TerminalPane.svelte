<script lang="ts">
	// Read-only live terminal (CCT-545). Mounts xterm.js, tells the server to
	// watch the session's PTY while open, and writes relayed bytes straight into
	// the terminal. Nothing is stored: a fresh daemon attach repaints the current
	// screen on open, so a late viewer still sees the live frame. Never sends
	// input upstream — strictly a video feed.
	import { onMount } from 'svelte';
	import { ws } from '$lib/ws.svelte';

	// The daemon's held/viewer attach is fixed at 120x40 (ATTACH_COLS/ROWS); size
	// the viewport to match so the geometry never fights the PTY.
	const COLS = 120;
	const ROWS = 40;

	let { sessionId, onclose }: { sessionId: string; onclose: () => void } = $props();

	let host = $state<HTMLDivElement | null>(null);
	let live = $state(false);

	onMount(() => {
		let disposed = false;
		let term: import('@xterm/xterm').Terminal | null = null;
		let offPty: (() => void) | null = null;

		// xterm and its CSS are browser-only; adapter-static SSRs, so load lazily.
		void (async () => {
			const [{ Terminal }] = await Promise.all([
				import('@xterm/xterm'),
				import('@xterm/xterm/css/xterm.css')
			]);
			if (disposed || !host) return;
			term = new Terminal({
				cols: COLS,
				rows: ROWS,
				convertEol: false,
				disableStdin: true,
				scrollback: 1000,
				fontSize: 12,
				fontFamily: 'var(--font-mono, ui-monospace, monospace)',
				theme: { background: '#0b0e14' }
			});
			term.open(host);
			offPty = ws.onPty(sessionId, (bytes) => term?.write(bytes));
			ws.watchPty(sessionId);
			live = true;
		})();

		return () => {
			disposed = true;
			live = false;
			offPty?.();
			ws.unwatchPty(sessionId);
			term?.dispose();
		};
	});
</script>

<div class="term-pane">
	<div class="term-head">
		<span class="term-title">
			<span class="term-dot" class:on={live}></span>
			Terminal (read-only{live ? ', live' : '…'})
		</span>
		<button type="button" class="term-close" onclick={onclose} aria-label="Close terminal">✕</button>
	</div>
	<div class="term-host" bind:this={host}></div>
</div>

<style>
	.term-pane {
		display: flex;
		flex-direction: column;
		border: 1px solid var(--border-strong);
		border-radius: var(--r-md);
		margin: var(--sp-2) var(--sp-3);
		overflow: hidden;
		background: #0b0e14;
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
		background: var(--ok, #3fb950);
		box-shadow: 0 0 6px var(--ok, #3fb950);
	}
	.term-close {
		border: none;
		background: transparent;
		color: var(--text-muted);
		cursor: pointer;
		font-size: var(--fs-sm);
	}
	.term-host {
		padding: var(--sp-1);
		overflow: auto;
	}
</style>
