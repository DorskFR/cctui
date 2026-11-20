import { afterEach, describe, expect, it, vi } from 'vitest';
import { Dictation, SPEECH_RMS } from './dictation';

vi.mock('#lib/api.ts', () => ({ api: {} }));

function stubMic() {
	const track = { stop: vi.fn() };
	let grant: () => void = () => {};
	const asked: MediaStreamConstraints[] = [];
	const recorders: { start: ReturnType<typeof vi.fn> }[] = [];
	vi.stubGlobal('navigator', {
		mediaDevices: {
			getUserMedia: (c: MediaStreamConstraints) => {
				asked.push(c);
				return new Promise((r) => (grant = () => r({ getTracks: () => [track] })));
			}
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
	return { track, recorders, asked, grant: () => grant() };
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

	it('opens the microphone picked in settings', async () => {
		const mic = stubMic();
		const d = new Dictation(() => ({ micId: 'headset' }));
		const started = d.start();
		mic.grant();
		await started;
		expect(mic.asked[0].audio).toMatchObject({ deviceId: { ideal: 'headset' }, echoCancellation: true });
		d.cancel();
	});
});

function stubAudio(level: number) {
	const closed = vi.fn(async () => {});
	vi.stubGlobal(
		'AudioContext',
		function (this: Record<string, unknown>) {
			this.state = 'running';
			this.resume = async () => {};
			this.close = closed;
			this.createAnalyser = () => ({
				fftSize: 0,
				getFloatTimeDomainData: (f: Float32Array) => f.fill(level)
			});
			this.createMediaStreamSource = () => ({ connect: () => {} });
		}
	);
	return { closed };
}

describe('Dictation speech meter', () => {
	afterEach(() => {
		vi.useRealTimers();
		vi.unstubAllGlobals();
	});

	async function opened(level: number | null) {
		vi.useFakeTimers();
		const mic = stubMic();
		const audio = level === null ? null : stubAudio(level);
		const d = new Dictation();
		const started = d.start();
		mic.grant();
		await started;
		return { d, audio };
	}

	it('hears nothing while the room is quiet', async () => {
		const { d } = await opened(SPEECH_RMS / 4);
		vi.advanceTimersByTime(1000);
		expect(d.heard).toBe(false);
		d.cancel();
	});

	it('hears a voice once the level stays up', async () => {
		const { d, audio } = await opened(SPEECH_RMS * 4);
		expect(d.heard).toBe(false);
		vi.advanceTimersByTime(300);
		expect(d.heard).toBe(true);
		d.cancel();
		expect(audio?.closed).toHaveBeenCalled();
	});

	it('counts as heard where the level cannot be read', async () => {
		const { d } = await opened(null);
		expect(d.heard).toBe(true);
		d.cancel();
	});
});
