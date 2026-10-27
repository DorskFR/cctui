/** Browser voice-activity detection on top of `@ricky0123/vad-web` (Silero),
 *  loaded on first use so pages that never go live pay nothing. The model
 *  and the onnxruntime wasm are served from `/vad/`, copied out of
 *  node_modules by `scripts/copy-vad-assets.mjs`. */

export interface VadCallbacks {
	onSpeechStart(): void;
	onSpeechEnd(): void;
	/** Speech too short to be an utterance; its frames are to be dropped. */
	onMisfire(): void;
	/** Every 16 kHz mono frame, speaking or not, with its speech probability. */
	onFrame(frame: Float32Array, speechProb: number): void;
}

export interface VadHandle {
	start(): Promise<void> | void;
	pause(): void;
	destroy(): void;
}

export type VadFactory = (cb: VadCallbacks) => Promise<VadHandle>;

import { micConstraints } from './devices';

export const VAD_ASSET_PATH = '/vad/';

export interface OrtEnvLike {
	logLevel?: string;
	wasm: { numThreads?: number };
}

/** onnxruntime-web ships only the threaded wasm build; its threads need
 *  SharedArrayBuffer, which a page without cross-origin isolation lacks. */
export function configureOrt(env: OrtEnvLike, isolated: boolean): void {
	env.logLevel = 'error';
	if (!isolated) env.wasm.numThreads = 1;
}

export const createVad = async (cb: VadCallbacks, micId?: string): Promise<VadHandle> => {
	const { MicVAD } = await import('@ricky0123/vad-web');
	const open = () => navigator.mediaDevices.getUserMedia({ audio: micConstraints(micId) });
	const vad = await MicVAD.new({
		getStream: open,
		resumeStream: open,
		model: 'v5',
		baseAssetPath: VAD_ASSET_PATH,
		onnxWASMBasePath: VAD_ASSET_PATH,
		ortConfig: (ort) => configureOrt(ort.env, globalThis.crossOriginIsolated === true),
		onSpeechStart: () => cb.onSpeechStart(),
		onSpeechEnd: () => cb.onSpeechEnd(),
		onVADMisfire: () => cb.onMisfire(),
		onFrameProcessed: (probs: { isSpeech: number }, frame: Float32Array) =>
			cb.onFrame(frame, probs.isSpeech)
	});
	return {
		start: () => vad.start(),
		pause: () => vad.pause(),
		destroy: () => vad.destroy()
	};
};
