import { defineJourney } from '@dorsk/journey';

// Tsumikit owns the theme picker, so its trigger is addressed by the component
// marker it renders rather than by a `data-journey` path of ours.
const THEMES = { css: '[data-tsu="ThemePicker"]' } as const;

export default defineJourney({
	id: 'sessions-list',
	title: 'Read the fleet at a glance',
	description: 'The sessions list groups every agent by what it needs from you.',
	route: '/sessions',
	variants: { viewport: ['desktop', 'mobile'], theme: ['dark', 'light', 'gruvbox'] },
	level: 'checked',
	steps: [
		{
			id: 'list',
			route: '/sessions',
			target: 'section[blocked]',
			say: {
				title: 'Every session, grouped',
				body: 'Sessions group by what they need — pinned first, then anything waiting on you.'
			},
			expect: [
				{ visible: { role: 'heading', name: 'Sessions' } },
				{ count: ['session', { min: 4 }] },
				{ visible: 'section[blocked]' }
			],
			capture: 'list'
		},
		{
			id: 'themes',
			target: THEMES,
			do: { kind: 'click' },
			say: {
				title: 'Every screen, in your palette',
				body: 'Twenty-one built-in themes, light and dark; the whole interface follows the swatch you pick.'
			},
			expect: [{ visible: { role: 'group', name: 'dark themes' } }],
			capture: 'themes'
		}
	]
});
