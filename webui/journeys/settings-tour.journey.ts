import { defineJourney } from '@dorsk/journey';

export default defineJourney({
	id: 'settings-tour',
	title: 'Tune how agents behave',
	description: 'Settings cover the look of the app, how sessions run, and what never leaves the machine.',
	route: '/settings/appearance',
	variants: { viewport: ['desktop', 'mobile'], theme: ['dark'] },
	level: 'checked',
	steps: [
		{
			id: 'appearance',
			route: '/settings/appearance',
			target: 'page[appearance]',
			say: {
				title: 'Make it yours',
				body: 'Theme, font size, language, and where the navigation sits — the whole interface follows the theme you pick.'
			},
			expect: [{ visible: 'page[appearance]' }, { visible: 'theme' }],
			capture: 'appearance'
		},
		{
			id: 'sessions',
			route: '/settings/sessions',
			target: 'page[sessions]',
			say: {
				title: 'Defaults for every run',
				body: 'How the list sorts and groups, and what a new session starts with, so you set it once instead of every time.'
			},
			expect: [{ visible: 'page[sessions]' }],
			capture: 'sessions'
		},
		{
			id: 'execution',
			route: '/settings/execution',
			target: 'page[execution]',
			say: {
				title: 'How much rope an agent gets',
				body: 'Permission handling, auto-approval and the phrases that mean an agent has stopped early rather than finished.'
			},
			expect: [{ visible: 'page[execution]' }],
			capture: 'execution'
		},
		{
			id: 'privacy',
			route: '/settings/privacy',
			target: 'page[privacy]',
			say: {
				title: 'Secrets never reach the transcript',
				body: 'Tokens and keys are detected and replaced before anything is stored, and you can add patterns of your own.'
			},
			expect: [{ visible: 'page[privacy]' }],
			capture: 'privacy'
		}
	]
});
