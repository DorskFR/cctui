export interface AudioDevice {
	id: string;
	label: string;
}

/** `ideal`, not `exact`: a headset switched off falls back to the default mic. */
export function micConstraints(deviceId?: string): MediaTrackConstraints {
	return {
		channelCount: 1,
		echoCancellation: true,
		autoGainControl: true,
		noiseSuppression: true,
		...(deviceId ? { deviceId: { ideal: deviceId } } : {})
	};
}

/** Labels stay empty until the page has had microphone access once. */
export async function listDevices(
	kind: 'audioinput' | 'audiooutput',
	media: Pick<MediaDevices, 'enumerateDevices'> | undefined = globalThis.navigator?.mediaDevices
): Promise<AudioDevice[]> {
	const all = (await media?.enumerateDevices().catch(() => [])) ?? [];
	return all
		.filter((d) => d.kind === kind && d.deviceId && d.deviceId !== 'default')
		.map((d, i) => ({ id: d.deviceId, label: d.label || `${kind === 'audioinput' ? '🎙' : '🔊'} ${i + 1}` }));
}

export const canPickSpeaker = (): boolean =>
	typeof HTMLMediaElement !== 'undefined' && 'setSinkId' in HTMLMediaElement.prototype;

export function routeToSpeaker(audio: object, deviceId?: string): void {
	const el = audio as { setSinkId?: (id: string) => Promise<void> };
	if (deviceId && el.setSinkId) void el.setSinkId(deviceId).catch(() => {});
}
