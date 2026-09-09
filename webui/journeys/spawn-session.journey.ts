import { defineJourney, param } from '@dorsk/journey';

export default defineJourney({
	id: 'spawn-session',
	title: 'Start a new agent',
	description: 'Describe the work, pick where it runs, and keep it as a draft until you are ready.',
	route: '/sessions',
	variants: { viewport: ['desktop', 'mobile'], theme: ['dark'] },
	level: 'checked',
	steps: [
		{
			id: 'open',
			route: '/sessions',
			target: 'new',
			do: { kind: 'click' },
			say: {
				title: 'Open the new-session dialog',
				body: 'Everything a run needs is in this one dialog: the machine, the folder, the prompt and the profile.'
			},
			expect: [{ visible: 'spawn' }, { visible: 'spawn/prompt' }],
			capture: 'dialog'
		},
		{
			id: 'where',
			target: 'where',
			say: {
				title: 'Pick where it runs',
				body: 'Pick the machine you enrolled and type the folder the agent should work in. The draft button lights up once both are set.'
			},
			expect: [{ enabled: 'draft' }]
		},
		{
			id: 'name',
			target: 'spawn/label',
			do: { kind: 'fill', value: param('var.label') },
			say: {
				title: 'Name the run',
				body: 'Give the run a name you will recognise in the list.'
			}
		},
		{
			id: 'prompt',
			target: 'spawn/prompt',
			do: { kind: 'fill', value: param('var.prompt') },
			say: {
				title: 'Say what you want done',
				body: 'Say what you want done. The profile below decides which harness and model carry it out.'
			},
			expect: [{ enabled: 'draft' }],
			capture: 'filled'
		},
		{
			id: 'save',
			target: 'draft',
			do: { kind: 'click' },
			say: {
				title: 'Save it as a draft',
				body: 'This saves a draft on your instance. Nothing runs until you launch it, and you can delete it from the list.'
			},
			expect: [{ hidden: 'spawn' }, { probe: 'sessions.drafts' }],
			capture: 'saved'
		},
		{
			id: 'sections',
			optional: true,
			target: 'sections/toggle',
			do: { kind: 'click' },
			say: {
				title: 'Choose what the list shows',
				body: 'The list is split into sections you can switch on and off; drafts are hidden until you ask for them.'
			},
			expect: [{ visible: 'sections/option[drafts]' }]
		},
		{
			id: 'show-drafts',
			optional: true,
			target: 'sections/option[drafts]',
			do: { kind: 'click' },
			say: {
				title: 'The draft is waiting',
				body: 'Your draft is here, holding the machine, folder, profile and prompt until you launch it.'
			},
			expect: [
				{ visible: 'section[drafts]' },
				{ count: ['section[drafts]/session', { min: 1 }] },
				{ probe: 'sessions.drafts' }
			],
			capture: 'draft'
		}
	]
});
