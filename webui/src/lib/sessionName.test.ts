import { describe, expect, it } from 'vitest';
import type { SessionListItem } from '@bindings/SessionListItem';
import { displayNameOf, sessionDisplayName } from './sessionName';

describe('sessionDisplayName', () => {
	it('shows the agent title beside a pinned name', () => {
		expect(sessionDisplayName('SUPER ORCHESTRATOR', 'super orchestrator resume', 'user')).toBe(
			'SUPER ORCHESTRATOR (super orchestrator resume)'
		);
	});

	it('shows a pinned name alone when the agent title repeats it', () => {
		expect(sessionDisplayName('Deploy prod', 'deploy prod', 'user')).toBe('Deploy prod');
		expect(sessionDisplayName('Deploy prod', ' ', 'user')).toBe('Deploy prod');
		expect(sessionDisplayName('Deploy prod', null, 'user')).toBe('Deploy prod');
	});

	it('shows an agent-owned name as it is', () => {
		expect(sessionDisplayName('🔧 fix the parser', '🔧 fix the parser', null)).toBe(
			'🔧 fix the parser'
		);
		expect(sessionDisplayName('typed long ago', 'old title', undefined)).toBe('typed long ago');
	});

	it('is empty with no name, unless a pinned session only has an agent title', () => {
		expect(sessionDisplayName(null, 'stray', null)).toBe('');
		expect(sessionDisplayName('  ', null, 'user')).toBe('');
		expect(sessionDisplayName('', 'agent title', 'user')).toBe('agent title');
	});
});

describe('displayNameOf', () => {
	const row = (name: string | null, metadata: unknown) =>
		({ name, metadata }) as Pick<SessionListItem, 'name' | 'metadata'>;

	it('reads the agent title and the name source from the metadata', () => {
		expect(
			displayNameOf(row('SUPER ORCHESTRATOR', { name_source: 'user', agent_title: 'resume' }))
		).toBe('SUPER ORCHESTRATOR (resume)');
		expect(displayNameOf(row('agent named', { agent_title: 'agent named' }))).toBe('agent named');
	});

	it('tolerates missing or malformed metadata', () => {
		expect(displayNameOf(row('n', null))).toBe('n');
		expect(displayNameOf(row('n', undefined))).toBe('n');
		expect(displayNameOf(row('n', ['user']))).toBe('n');
		expect(displayNameOf(row('n', { name_source: 'user', agent_title: 7 }))).toBe('n');
	});
});
