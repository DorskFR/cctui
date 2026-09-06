import { defineJourney } from '@dorsk/journey';

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
				body: 'One dialog covers everything a run needs: the machine, the folder, the prompt and the profile. It opens on the folder you used last.'
			},
			expect: [
				{ visible: 'spawn' },
				{ visible: 'spawn/prompt' },
				{ enabled: 'draft' }
			],
			capture: 'dialog'
		},
		{
			id: 'name',
			target: 'spawn/label',
			do: { kind: 'fill', value: 'Add pagination to the orders endpoint' },
			say: {
				title: 'Name the run',
				body: 'The label is what you will look for in the list later, so give it the shape of the task.'
			},
			expect: [{ value: ['spawn/label', 'Add pagination to the orders endpoint'] }]
		},
		{
			id: 'prompt',
			target: 'spawn/prompt',
			do: {
				kind: 'fill',
				value: 'Add cursor pagination to GET /orders. Keep the response shape and cover it with a test.'
			},
			say: {
				title: 'Say what you want done',
				body: 'The prompt is the whole brief; the profile below it decides which harness, model and folder carry it out.'
			},
			expect: [
				{
					value: [
						'spawn/prompt',
						'Add cursor pagination to GET /orders. Keep the response shape and cover it with a test.'
					]
				},
				{ enabled: 'draft' }
			],
			capture: 'filled'
		},
		{
			id: 'save',
			target: 'draft',
			do: { kind: 'click' },
			say: {
				title: 'Keep it for later',
				body: 'Saving a draft parks the whole configuration in the list, ready to launch when you are.'
			},
			expect: [{ hidden: 'spawn' }],
			capture: 'saved'
		},
		{
			id: 'sections',
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
			target: 'sections/option[drafts]',
			do: { kind: 'click' },
			say: {
				title: 'The draft is waiting',
				body: 'Drafts sit in their own section, holding the machine, folder, profile and prompt until you launch them.'
			},
			expect: [{ visible: 'section[drafts]' }, { count: ['section[drafts]/session', { min: 1 }] }],
			capture: 'draft'
		}
	]
});
