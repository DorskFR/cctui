import { describe, expect, it, vi } from 'vitest';
import { listDevices, micConstraints, routeToSpeaker } from './devices';

const device = (kind: MediaDeviceKind, deviceId: string, label = '') =>
	({ kind, deviceId, label, groupId: '' }) as MediaDeviceInfo;

describe('audio devices', () => {
	it('asks for the picked mic without failing when it is gone', () => {
		expect(micConstraints()).not.toHaveProperty('deviceId');
		expect(micConstraints('bt')).toMatchObject({ deviceId: { ideal: 'bt' }, echoCancellation: true });
	});

	it('lists one kind, skips the default alias and numbers unnamed devices', async () => {
		const media = {
			enumerateDevices: async () => [
				device('audioinput', 'default', 'Default'),
				device('audioinput', 'a', 'Laptop mic'),
				device('audioinput', 'b'),
				device('audiooutput', 'c', 'Headphones')
			]
		};
		expect(await listDevices('audioinput', media)).toEqual([
			{ id: 'a', label: 'Laptop mic' },
			{ id: 'b', label: '🎙 2' }
		]);
		expect(await listDevices('audiooutput', media)).toEqual([{ id: 'c', label: 'Headphones' }]);
		expect(await listDevices('audioinput', undefined)).toEqual([]);
	});

	it('routes playback only when a speaker is picked and the browser can', () => {
		const setSinkId = vi.fn(async () => {});
		routeToSpeaker({ setSinkId });
		expect(setSinkId).not.toHaveBeenCalled();
		routeToSpeaker({ setSinkId }, 'bt');
		expect(setSinkId).toHaveBeenCalledWith('bt');
		expect(() => routeToSpeaker({}, 'bt')).not.toThrow();
	});
});
