import { describe, expect, it } from 'vitest';
import { configureOrt, type OrtEnvLike } from './vad';

describe('configureOrt', () => {
	it('pins a single wasm thread when the page is not cross-origin isolated', () => {
		const env: OrtEnvLike = { wasm: {} };
		configureOrt(env, false);
		expect(env.wasm.numThreads).toBe(1);
		expect(env.logLevel).toBe('error');
	});

	it('leaves the thread count to onnxruntime when isolated', () => {
		const env: OrtEnvLike = { wasm: {} };
		configureOrt(env, true);
		expect(env.wasm.numThreads).toBeUndefined();
		expect(env.logLevel).toBe('error');
	});
});
