/** Plays synthesized sentences in arrival order. Each `speak_start` opens a
 *  line that collects binary chunks until its `speak_end`, then joins the
 *  queue; one `<audio>` element plays them back to back. */

export interface AudioLike {
	src: string;
	playbackRate: number;
	onended: ((ev: Event) => void) | null;
	onerror: ((ev: Event | string) => void) | null;
	play(): Promise<void>;
	pause(): void;
}

export interface PlayerOptions {
	mime: () => string;
	speed?: () => number;
	/** `true` once playback has nothing left to do. */
	onIdle?: (idle: boolean) => void;
	createAudio?: () => AudioLike;
	createUrl?: (blob: Blob) => string;
	revokeUrl?: (url: string) => void;
}

interface Pending {
	lineId: string;
	chunks: Uint8Array[];
}

export class SpeechPlayer {
	private open: Pending | null = null;
	private queue: Blob[] = [];
	private current: AudioLike | null = null;
	private currentUrl: string | null = null;
	private unlocked = false;
	private readonly opts: PlayerOptions;

	constructor(opts: PlayerOptions) {
		this.opts = opts;
	}

	get playing(): boolean {
		return this.current !== null;
	}

	/** Call from a user gesture: iOS only lets a tapped element start audio,
	 *  and one silent play per element keeps it allowed afterwards. */
	async unlock(): Promise<void> {
		if (this.unlocked) return;
		this.unlocked = true;
		try {
			const a = this.audio();
			a.src = 'data:audio/wav;base64,UklGRiQAAABXQVZFZm10IBAAAAABAAEAQB8AAIA+AAACABAAZGF0YQAAAAA=';
			await a.play();
			a.pause();
		} catch {
			/* a locked element just fails on the first real play instead */
		}
	}

	begin(lineId: string) {
		this.open = { lineId, chunks: [] };
	}

	chunk(bytes: Uint8Array) {
		this.open?.chunks.push(bytes);
	}

	end(lineId: string) {
		const line = this.open;
		this.open = null;
		if (!line || line.lineId !== lineId || !line.chunks.length) return;
		this.queue.push(new Blob(line.chunks as BlobPart[], { type: this.opts.mime() }));
		if (!this.current) this.next();
	}

	/** Drop everything queued and stop what is playing. */
	stop() {
		this.open = null;
		this.queue = [];
		const cur = this.current;
		this.current = null;
		if (cur) {
			cur.onended = null;
			cur.onerror = null;
			cur.pause();
		}
		this.releaseUrl();
		this.opts.onIdle?.(true);
	}

	private audio(): AudioLike {
		const a = this.current ?? this.opts.createAudio?.() ?? new Audio();
		this.current = a;
		return a;
	}

	private next() {
		const blob = this.queue.shift();
		if (!blob) {
			this.current = null;
			this.releaseUrl();
			this.opts.onIdle?.(true);
			return;
		}
		this.opts.onIdle?.(false);
		const a = this.opts.createAudio?.() ?? new Audio();
		this.current = a;
		this.releaseUrl();
		this.currentUrl = this.opts.createUrl?.(blob) ?? URL.createObjectURL(blob);
		a.src = this.currentUrl;
		a.playbackRate = this.opts.speed?.() ?? 1;
		a.onended = () => this.next();
		a.onerror = () => this.next();
		void a.play().catch(() => this.next());
	}

	private releaseUrl() {
		if (!this.currentUrl) return;
		(this.opts.revokeUrl ?? URL.revokeObjectURL)(this.currentUrl);
		this.currentUrl = null;
	}
}
