// Unit tests never reach a server: an unstubbed fetch rejects and an unstubbed
// WebSocket never opens. Tests that need either install their own fake.
globalThis.fetch = (input) =>
	Promise.reject(new TypeError(`unstubbed fetch in a unit test: ${String(input)}`));

class InertWebSocket extends EventTarget {
	static readonly CONNECTING = 0;
	static readonly OPEN = 1;
	static readonly CLOSING = 2;
	static readonly CLOSED = 3;
	readonly CONNECTING = 0;
	readonly OPEN = 1;
	readonly CLOSING = 2;
	readonly CLOSED = 3;
	readyState = 0;
	binaryType = 'blob';
	bufferedAmount = 0;
	extensions = '';
	protocol = '';
	onopen: ((ev: Event) => void) | null = null;
	onclose: ((ev: Event) => void) | null = null;
	onerror: ((ev: Event) => void) | null = null;
	onmessage: ((ev: Event) => void) | null = null;
	constructor(readonly url: string | URL) {
		super();
	}
	send(): void {}
	close(): void {
		this.readyState = 3;
	}
}
globalThis.WebSocket = InertWebSocket as unknown as typeof WebSocket;
