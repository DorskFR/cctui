import { describe, expect, it } from 'vitest';
import { isMachineUuid } from './lineRender.svelte';

describe('isMachineUuid', () => {
	it('accepts a canonical uuid in either case', () => {
		expect(isMachineUuid('0f8fad5b-d9cb-469f-a165-70867728950e')).toBe(true);
		expect(isMachineUuid('0F8FAD5B-D9CB-469F-A165-70867728950E')).toBe(true);
	});

	it('rejects legacy hostname-valued machine ids', () => {
		expect(isMachineUuid('homelab-worker-1')).toBe(false);
		expect(isMachineUuid('')).toBe(false);
		expect(isMachineUuid('0f8fad5b-d9cb-469f-a165-70867728950e-x')).toBe(false);
	});
});
