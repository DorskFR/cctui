import { defineJourney } from '@dorsk/journey';

const ACP_SESSION = 'session[a0000000-0000-4000-8000-000000000006]/title';
const FIRST_TOOL_LINE = {
	css: '[data-journey="conversation"] [data-journey="line"][data-journey-key="tool"]',
	nth: 0
} as const;
const SESSIONS = '/sessions';

export default defineJourney({
	id: 'acp-session',
	title: { en: 'A session on an ACP harness', fr: 'Une session sur un harnais ACP' },
	description: {
		en: 'A gemini session driven over the Agent Client Protocol reads like any other: same header, same timeline, same tool calls.',
		fr: 'Une session gemini pilotée par l’Agent Client Protocol se lit comme les autres : même en-tête, même chronologie, mêmes appels d’outils.'
	},
	route: SESSIONS,
	variants: { viewport: ['desktop', 'mobile'], theme: ['dark'] },
	level: 'checked',
	steps: [
		{
			id: 'open',
			qaOnly: true,
			route: SESSIONS,
			target: ACP_SESSION,
			do: { kind: 'click' },
			say: {
				title: { en: 'Open the gemini session', fr: 'Ouvrir la session gemini' },
				body: {
					en: 'It sits in the list beside the Claude Code and Codex sessions, badged with its own harness.',
					fr: 'Elle figure dans la liste à côté des sessions Claude Code et Codex, avec le badge de son propre harnais.'
				}
			},
			expect: [{ visible: 'conversation' }],
			capture: 'drawer'
		},
		{
			id: 'header',
			qaOnly: true,
			target: 'conversation/header',
			say: {
				title: { en: 'The harness is named in the header', fr: 'Le harnais est nommé dans l’en-tête' },
				body: {
					en: 'Machine, model and harness read the same as for a native adapter.',
					fr: 'Machine, modèle et harnais se lisent comme pour un adaptateur natif.'
				}
			},
			expect: [{ visible: 'conversation/header' }],
			capture: 'header'
		},
		{
			id: 'tools',
			qaOnly: true,
			target: FIRST_TOOL_LINE,
			say: {
				title: { en: 'ACP tool calls in the timeline', fr: 'Les appels d’outils ACP dans la chronologie' },
				body: {
					en: 'The agent’s own tools (read_file, run_shell_command) show up as tool lines with their results.',
					fr: 'Les outils propres à l’agent (read_file, run_shell_command) apparaissent comme lignes d’outil avec leurs résultats.'
				}
			},
			expect: [{ visible: FIRST_TOOL_LINE }],
			capture: 'tools'
		}
	]
});
