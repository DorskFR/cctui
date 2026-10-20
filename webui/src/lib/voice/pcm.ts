/** 16-bit little-endian PCM of a float sample buffer, as the voice socket
 *  expects an utterance. */
export function floatToPcm16(samples: Float32Array): ArrayBuffer {
	const out = new DataView(new ArrayBuffer(samples.length * 2));
	for (let i = 0; i < samples.length; i++) {
		const s = Math.max(-1, Math.min(1, samples[i]));
		out.setInt16(i * 2, s < 0 ? s * 0x8000 : s * 0x7fff, true);
	}
	return out.buffer;
}

/** Root-mean-square level of a frame, 0..1, for the live meter. */
export function frameLevel(frame: Float32Array): number {
	if (!frame.length) return 0;
	let sum = 0;
	for (let i = 0; i < frame.length; i++) sum += frame[i] * frame[i];
	return Math.min(1, Math.sqrt(sum / frame.length) * 4);
}
