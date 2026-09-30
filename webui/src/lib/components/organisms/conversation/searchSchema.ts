import type { Schema, ValueOption } from '@dorsk/tsumikit';
import { m } from '$lib/paraglide/messages';
import { MSG_GROUPS } from './filters';
import { msgCategoryLabel, msgGroupLabel } from './types';

/** The find-in-conversation placeholder, re-read per render so a locale switch
 *  lands. */
export const conversationSearchPlaceholder = (): string => m.conversation_search_placeholder();

/** Categories in `MSG_GROUPS` order, each labelled with its group so the
 *  dropdown reads the way the toolbar's filter menu does. */
function roleOptions(): ValueOption[] {
	return MSG_GROUPS.flatMap((g) =>
		g.categories.map((c) => ({
			value: c,
			label: msgCategoryLabel(c),
			hint: msgGroupLabel(g.id)
		}))
	);
}

/**
 * The transcript's own filter schema. Field names match what
 * `GET /sessions/{id}/search` parses through `cctui_query`: `role` (alias
 * `type`), `tool`, `after`, `before`, `pinned`, plus free text.
 *
 * `after`/`before` are separate fields rather than one `created` with `gte`/`lte`
 * because the server's query AST carries no comparison operators — only
 * `eq`/`ne`/`in`/`contains`.
 *
 * `tools` supplies the `tool:` autocomplete: the tool ids this session actually
 * used, so the list is never a global catalogue.
 */
export function buildConversationSearchSchema(tools: () => string[]): Schema {
	return {
		fields: [
			{
				name: 'role',
				label: m.conversation_search_field_role(),
				type: 'enum',
				aliases: ['type'],
				operators: ['eq', 'ne', 'in'],
				options: roleOptions()
			},
			{
				name: 'tool',
				label: m.conversation_search_field_tool(),
				type: 'string',
				operators: ['eq', 'ne', 'in'],
				valuePlaceholder: 'Bash',
				provider: (q: string) => {
					const needle = q.trim().toLowerCase();
					return tools()
						.filter((t) => !needle || t.toLowerCase().includes(needle))
						.map((t) => ({ value: t, label: t }));
				}
			},
			{
				name: 'after',
				label: m.conversation_search_field_after_date(),
				type: 'date',
				operators: ['eq'],
				valuePlaceholder: 'YYYY-MM-DD'
			},
			{
				name: 'before',
				label: m.conversation_search_field_before_date(),
				type: 'date',
				operators: ['eq'],
				valuePlaceholder: 'YYYY-MM-DD'
			},
			{
				name: 'pinned',
				label: m.search_field_pinned(),
				type: 'bool',
				operators: ['eq'],
				options: [
					{ value: 'true', label: 'true' },
					{ value: 'false', label: 'false' }
				]
			}
		]
	};
}
