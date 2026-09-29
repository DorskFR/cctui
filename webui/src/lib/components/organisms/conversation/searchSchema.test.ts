import { describe, expect, it } from 'vitest';
import { filters, freeText, parse } from '@dorsk/tsumikit';
import { buildConversationSearchSchema } from './searchSchema';
import { MSG_CATEGORIES } from './filters';

const schema = buildConversationSearchSchema(() => ['Bash', 'Read', 'mcp__github__issue']);
const field = (name: string) => schema.fields.find((f) => f.name === name);

describe('buildConversationSearchSchema', () => {
	it('covers exactly the fields the server parses', () => {
		expect(schema.fields.map((f) => f.name)).toEqual([
			'role',
			'tool',
			'after',
			'before',
			'pinned'
		]);
	});

	it('offers every message category as a role, `type` included as an alias', () => {
		const role = field('role');
		expect(role?.aliases).toContain('type');
		expect(role?.options?.map((o) => o.value)).toEqual(MSG_CATEGORIES);
	});

	it('parses role, tool, date and pinned clauses out of the free text', () => {
		const raw = 'role:user tool:Bash after:2026-09-01 pinned:true needle "two words"';
		const ast = parse(raw, schema);
		const byField = Object.fromEntries(filters(ast).map((f) => [f.field, f.values]));
		expect(byField).toMatchObject({
			role: ['user'],
			tool: ['Bash'],
			after: ['2026-09-01'],
			pinned: ['true']
		});
		expect(freeText(ast)).toContain('needle');
		expect(freeText(ast)).toContain('two words');
	});

	it('autocompletes tools from the session, not a global catalogue', async () => {
		const provider = field('tool')?.provider;
		if (!provider) throw new Error('tool field has no value provider');
		expect(await provider('mcp')).toEqual([
			{ value: 'mcp__github__issue', label: 'mcp__github__issue' }
		]);
		expect((await provider('')).length).toBe(3);
	});

	it('gives after/before one operator each: the server AST has no comparisons', () => {
		expect(field('after')?.operators).toEqual(['eq']);
		expect(field('before')?.operators).toEqual(['eq']);
	});
});
