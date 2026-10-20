import { afterEach, describe, expect, it, vi } from 'vitest';
import { Dictation } from './dictation';

vi.mock('$lib/api', () => ({ api: {} }));

function stubMic() {
	const track = { stop: vi.fn() };
	let grant: () => void = () => {};
	const recorders: { start: ReturnType<typeof vi.fn> }[] = [];
	vi.stubGlobal('navigator', {
		mediaDevices: {
			getUserMedia: () =>
				new Promise((r) => (grant = () => r({ getTracks: () => [track] })))
		}
	});
	vi.stubGlobal(
		'MediaRecorder',
		Object.assign(
			function (this: { start: unknown; stop: unknown; state: string }) {
				this.start = vi.fn(() => (this.state = 'recording'));
				this.stop = vi.fn(() => (this.state = 'inactive'));
				this.state = 'inactive';
				recorders.push(this as never);
			},
			{ isTypeSupported: () => false }
		)
	);
	return { track, recorders, grant: () => grant() };
}

describe('Dictation', () => {
	afterEach(() => vi.unstubAllGlobals());

	it('never records when cancelled before the microphone opens', async () => {
		const mic = stubMic();
		const d = new Dictation();
		const started = d.start();
		d.cancel();
		mic.grant();
		await started;
		expect(d.recording).toBe(false);
		expect(mic.recorders).toHaveLength(0);
		expect(mic.track.stop).toHaveBeenCalled();
	});

	it('records once the microphone opens', async () => {
		const mic = stubMic();
		const d = new Dictation();
		const started = d.start();
		mic.grant();
		await started;
		expect(d.recording).toBe(true);
		d.cancel();
		expect(mic.track.stop).toHaveBeenCalled();
	});
});
