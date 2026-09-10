import { defineJourney } from '@dorsk/journey';

// Tsumikit owns the theme picker, so its trigger is addressed by the component
// marker it renders rather than by a `data-journey` path of ours.
const THEMES = { css: '[data-tsu="ThemePicker"]' } as const;

export default defineJourney({
	id: 'sessions-list',
	title: { en: 'Read the fleet at a glance', fr: 'Lire la flotte d’un coup d’œil' },
	description: { en: 'The sessions list groups every agent by what it needs from you.', fr: 'La liste des sessions regroupe chaque agent selon ce qu’il attend de vous.' },
	route: '/sessions',
	variants: { viewport: ['desktop', 'mobile'], theme: ['dark', 'light', 'gruvbox'] },
	level: 'checked',
	steps: [
		{
			id: 'list',
			route: '/sessions',
			target: 'sections',
			say: {
				title: 'Every session, grouped',
				body: 'Sessions group by what they need from you: pinned first, then anything waiting on an answer. This filter chooses which groups are shown.'
			},
			expect: [{ visible: { role: 'heading', name: 'Sessions' } }, { visible: 'sections' }]
		},
		{
			id: 'list-fixture',
			qaOnly: true,
			target: 'section[blocked]',
			expect: [{ count: ['session', { min: 4 }] }, { visible: 'section[blocked]' }],
			capture: 'list'
		},
		{
			id: 'themes',
			target: THEMES,
			do: { kind: 'click' },
			say: {
				title: { en: 'Every screen, in your palette', fr: 'Chaque écran, dans votre palette' },
				body: { en: 'Twenty-one built-in themes, light and dark; the whole interface follows the swatch you pick.', fr: 'Vingt et un thèmes intégrés, clairs et sombres ; toute l’interface suit la teinte que vous choisissez.' }
			},
			expect: [{ visible: { role: 'group', name: 'dark themes' } }],
			capture: 'themes'
		}
	]
});
