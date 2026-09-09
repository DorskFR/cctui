import { defineJourney } from '@dorsk/journey';

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
			target: 'session[{session}]/title',
			do: { kind: 'click' },
			say: {
				title: 'Open a session',
				body: 'Open your running session by its name. The conversation opens beside the list on a desktop, over it on a phone.'
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
			expect: [{ visible: 'conversation' }],
			capture: 'timeline'
		},
		{
			id: 'mobile-filters',
			when: { viewport: 'mobile' },
			target: 'mobile-panel[filters]',
			do: { kind: 'click' },
			say: { title: 'Open the filters' },
			expect: [{ visible: 'filters/quick[assistant]' }]
		},
		{
			id: 'filters',
			target: 'filters',
			say: {
				title: 'Filter the noise',
				body: 'These pills hide message kinds. Turning off assistant messages leaves the tool calls, the quickest way to see what an agent touched.'
			},
			expect: [{ visible: 'filters/quick[assistant]' }]
		},
		{
			id: 'tools-only',
			qaOnly: true,
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
			say: {
				title: 'Steer it from here',
				body: 'Anything you type here goes to the running agent, so you can redirect it without restarting.'
			},
			expect: [{ visible: 'composer/message' }],
			capture: 'reply'
		}
	]
});
