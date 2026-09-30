import { defineJourney } from '@dorsk/journey';

const ROUTE = '/apps/pagedemo';

export default defineJourney({
	id: 'apps-page-plugin',
	title: { en: 'Use a plugin that owns a whole page', fr: 'Utiliser un plugin qui occupe une page entière' },
	description: {
		en: 'A page plugin gets its own nav entry and its own routes under /apps/<id>.',
		fr: 'Un plugin de page obtient son entrée de navigation et ses propres routes sous /apps/<id>.'
	},
	route: ROUTE,
	variants: { viewport: ['desktop', 'mobile'], theme: ['dark'] },
	level: 'checked',
	steps: [
		{
			id: 'page',
			route: ROUTE,
			qaOnly: true,
			target: 'pagedemo',
			say: {
				title: { en: 'The plugin owns the page', fr: 'Le plugin occupe la page' },
				body: {
					en: 'The host mounts the plugin under /apps/<id> and hands it the path below that prefix. Everything you see here is the plugin, not cctui.',
					fr: 'L’hôte monte le plugin sous /apps/<id> et lui transmet le chemin situé sous ce préfixe. Tout ce qui est affiché ici vient du plugin, pas de cctui.'
				}
			},
			expect: [{ visible: 'pagedemo' }, { visible: 'pagedemo-list' }]
		},
		{
			id: 'navigate',
			route: ROUTE,
			qaOnly: true,
			target: 'pagedemo-item[beta]',
			do: { kind: 'click' },
			say: {
				title: { en: 'Its own routes are real URLs', fr: 'Ses routes sont de vraies URL' },
				body: {
					en: 'The plugin calls the host to navigate, so the address bar, the browser back button and a bookmark all work on a plugin screen.',
					fr: 'Le plugin demande la navigation à l’hôte : la barre d’adresse, le bouton retour du navigateur et un favori fonctionnent donc sur un écran de plugin.'
				}
			},
			expect: [{ visible: 'pagedemo-detail' }, { visible: 'pagedemo-back' }]
		},
		{
			id: 'not-enabled',
			route: '/apps/pagedemo',
			qaOnly: true,
			optional: true,
			target: 'plugin-page-not-enabled',
			say: {
				title: { en: 'Off until you switch it on', fr: 'Désactivé jusqu’à ce que vous l’activiez' },
				body: {
					en: 'A page plugin follows the same two gates as a pane: an admin enables it for the instance, and you switch it on for yourself.',
					fr: 'Un plugin de page suit les deux mêmes conditions qu’un volet : un administrateur l’active pour l’instance, et vous l’activez pour vous-même.'
				}
			},
			expect: [{ visible: 'plugin-page-not-enabled' }]
		}
	]
});
