import { describe, expect, it } from 'vitest';
import type { SessionListItem } from '@bindings/SessionListItem';
import { displayNameOf, headerTitleOf, nameOf, sessionDisplayName } from './sessionName';

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

describe('nameOf', () => {
	const row = (name: string | null, metadata: unknown) =>
		({ name, metadata }) as Pick<SessionListItem, 'name' | 'metadata'>;

	it('prefers the stored name', () => {
		expect(nameOf(row(' lane d ', { name_source: 'user', custom_title: 'old' }))).toBe('lane d');
	});

	it('falls back to the pinned title when the stored name is gone', () => {
		expect(nameOf(row(null, { name_source: 'user', custom_title: 'qa-voice-0261b12' }))).toBe(
			'qa-voice-0261b12'
		);
		expect(nameOf(row('  ', { name_source: 'user', custom_title: 'pinned' }))).toBe('pinned');
	});

	it('ignores a custom title the user never pinned', () => {
		expect(nameOf(row(null, { custom_title: 'stray' }))).toBe('');
		expect(nameOf(row(null, null))).toBe('');
	});
});

describe('headerTitleOf', () => {
	const row = (name: string | null, metadata: unknown) =>
		({ name, metadata, working_dir: '/home/dorsk/Documents' }) as Pick<
			SessionListItem,
			'name' | 'metadata' | 'working_dir'
		>;

	it('shows the name of a named session, interactive or dispatched', () => {
		expect(headerTitleOf(row('qa-voice-0261b12', { name_source: 'user' }))).toBe(
			'qa-voice-0261b12'
		);
		expect(headerTitleOf(row('dispatched job', { agent_title: 'dispatched job' }))).toBe(
			'dispatched job'
		);
		expect(
			headerTitleOf(row(null, { name_source: 'user', custom_title: 'qa-voice-0261b12' }))
		).toBe('qa-voice-0261b12');
	});

	it('falls back to the working folder for an unnamed session', () => {
		expect(headerTitleOf(row(null, {}))).toBe('/home/dorsk/Documents');
		expect(headerTitleOf(row('  ', null))).toBe('/home/dorsk/Documents');
	});
});
