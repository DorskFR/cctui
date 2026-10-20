/** Browser voice-activity detection on top of `@ricky0123/vad-web` (Silero),
 *  loaded on first use so pages that never go live pay nothing. The model
 *  and the onnxruntime wasm are served from `/vad/`, copied out of
 *  node_modules by `scripts/copy-vad-assets.mjs`. */
import { frameLevel } from './pcm';

export interface VadCallbacks {
	onSpeechStart(): void;
	/** One utterance: 16 kHz mono float samples. */
	onSpeechEnd(audio: Float32Array): void;
	onLevel(level: number): void;
}

export interface VadHandle {
	start(): Promise<void> | void;
	pause(): void;
	destroy(): void;
}

export type VadFactory = (cb: VadCallbacks) => Promise<VadHandle>;

export const VAD_ASSET_PATH = '/vad/';

export const createVad: VadFactory = async (cb) => {
	const { MicVAD } = await import('@ricky0123/vad-web');
	const vad = await MicVAD.new({
		model: 'v5',
		baseAssetPath: VAD_ASSET_PATH,
		onnxWASMBasePath: VAD_ASSET_PATH,
		onSpeechStart: () => cb.onSpeechStart(),
		onSpeechEnd: (audio: Float32Array) => cb.onSpeechEnd(audio),
		onFrameProcessed: (_probs: unknown, frame: Float32Array) => cb.onLevel(frameLevel(frame))
	});
	return {
		start: () => vad.start(),
		pause: () => vad.pause(),
		destroy: () => vad.destroy()
	};
};
