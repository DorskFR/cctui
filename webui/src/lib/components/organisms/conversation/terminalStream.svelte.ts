/**
 * Buffers relayed PTY bytes until xterm exists.
 *
 * The watch is sent on mount, in parallel with the lazy xterm import and the
 * font load, so the daemon's repaint can land before there is a `Terminal` to
 * write it into. That repaint is the only frame that carries the current
 * screen, so it has to be held rather than dropped.
 */

export interface TerminalWriter {
	write(data: Uint8Array): void;
}

export class PtyStream {
	#buffered: Uint8Array[] = [];
	#writer: TerminalWriter | null = null;
	/** True from the first chunk, not from when the watch was sent. */
	live = $state(false);

	push = (data: Uint8Array): void => {
		this.live = true;
		if (this.#writer) this.#writer.write(data);
		else this.#buffered.push(data);
	};

	attach = (writer: TerminalWriter): void => {
		this.#writer = writer;
		const pending = this.#buffered;
		this.#buffered = [];
		for (const chunk of pending) writer.write(chunk);
	};

	detach = (): void => {
		this.#writer = null;
		this.#buffered = [];
		this.live = false;
	};
}

/**
 * Resolve once the bundled font is usable, or after `capMs` either way: xterm
 * measures cell width from whatever is loaded, and a wrong measurement is a
 * cosmetic glyph-spacing bug, while blocking on the font is a blank pane.
 */
export async function loadTerminalFont(font: string, capMs: number): Promise<void> {
	if (!('fonts' in document)) return;
	try {
		await Promise.race([
			document.fonts.load(`12px "${font}"`),
			new Promise((resolve) => setTimeout(resolve, capMs))
		]);
	} catch {
		/* fonts API rejected — fall through with the resolved stack */
	}
}
