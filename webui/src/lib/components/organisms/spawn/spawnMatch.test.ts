import { describe, expect, it } from 'vitest';
import { findSpawnedSession } from './spawnMatch';

const native = { id: 'native-thread', machine_id: 'machine', metadata: { spawn_key: 'command' } };

describe('findSpawnedSession', () => {
	it('finds a native thread by its dispatch key rather than its id', () => {
		expect(findSpawnedSession([native], 'machine', 'command', null)).toBe(native);
	});
	it('does not claim a concurrent spawn or a key from another machine', () => {
		expect(findSpawnedSession([native], 'machine', 'other-command', null)).toBeNull();
		expect(findSpawnedSession([native], 'other-machine', 'command', null)).toBeNull();
	});
	it('uses the exact pre-minted id when available', () => {
		expect(findSpawnedSession([native], 'machine', 'other-command', native.id)).toBe(native);
		expect(findSpawnedSession([native], 'machine', 'command', 'another-id')).toBeNull();
	});
	it.each([null, [], 'invalid', 42, {}])('ignores metadata without a spawn key: %j', (metadata) => {
		expect(findSpawnedSession([{ ...native, metadata }], 'machine', 'command', null)).toBeNull();
	});
});
