import { defineJourney } from '@dorsk/journey';

const SESSION = 'a0000000-0000-4000-8000-000000000001';

export default defineJourney({
	id: 'follow-session',
	title: 'Follow a session while it works',
	description: 'Open a running agent, read what it did, and reply without leaving the list.',
	route: '/sessions',
	variants: { viewport: ['desktop', 'mobile'], theme: ['dark'] },
	level: 'checked',
	steps: [
		{
			id: 'open',
			route: '/sessions',
			target: `session[${SESSION}]/title`,
			do: { kind: 'click' },
			say: {
				title: 'Open a session',
				body: 'Tapping a session by name opens its conversation — beside the list on a desktop, over it on a phone.'
			},
			expect: [{ visible: 'conversation' }, { visible: 'composer' }],
			capture: 'drawer'
		},
		{
			id: 'timeline',
			target: 'conversation',
			say: {
				title: 'Read the whole transcript',
				body: 'Every prompt, reply, tool call and tool result is kept, so you can see exactly what the agent did.'
			},
			expect: [{ count: ['conversation/line', { min: 5 }] }],
			capture: 'timeline'
		},
		{
			id: 'mobile-filters',
			when: { viewport: 'mobile' },
			target: 'mobile-panel[filters]',
			do: { kind: 'click' },
			expect: [{ visible: 'filters/quick[assistant]' }]
		},
		{
			id: 'tools-only',
			target: 'filters/quick[assistant]',
			do: { kind: 'click' },
			say: {
				title: 'Filter the noise',
				body: 'Hiding the assistant messages leaves the tool calls — the fastest way to audit what an agent touched.'
			},
			expect: [{ visible: 'conversation/line[tool]' }, { hidden: 'conversation/line[assistant]' }],
			capture: 'tools'
		},
		{
			id: 'reply',
			target: 'composer/message',
			do: { kind: 'fill', value: 'Also cover the empty-page case before you push.' },
			say: {
				title: 'Steer it mid-run',
				body: 'A reply goes to the running agent, so you can redirect the work without restarting it.'
			},
			expect: [{ value: ['composer/message', 'Also cover the empty-page case before you push.'] }],
			capture: 'reply'
		}
	]
});
