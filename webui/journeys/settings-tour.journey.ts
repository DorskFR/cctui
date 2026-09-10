import { defineJourney } from '@dorsk/journey';

export default defineJourney({
	id: 'settings-tour',
	title: { en: 'Tune how agents behave', fr: 'Régler le comportement des agents' },
	description: { en: 'Settings cover the look of the app, how sessions run, and what never leaves the machine.', fr: 'Les réglages couvrent l’apparence de l’application, le déroulement des sessions et ce qui ne quitte jamais la machine.' },
	route: '/settings/appearance',
	variants: { viewport: ['desktop', 'mobile'], theme: ['dark'] },
	level: 'checked',
	steps: [
		{
			id: 'appearance',
			route: '/settings/appearance',
			target: 'page[appearance]',
			say: {
				title: { en: 'Make it yours', fr: 'À votre image' },
				body: { en: 'Theme, font size, language, and where the navigation sits — the whole interface follows the theme you pick.', fr: 'Thème, taille du texte, langue et position de la navigation — toute l’interface suit le thème que vous choisissez.' }
			},
			expect: [{ visible: 'page[appearance]' }, { visible: 'theme' }],
			capture: 'appearance'
		},
		{
			id: 'sessions',
			route: '/settings/sessions',
			target: 'page[sessions]',
			say: {
				title: { en: 'Defaults for every run', fr: 'Les réglages par défaut de chaque run' },
				body: { en: 'How the list sorts and groups, and what a new session starts with, so you set it once instead of every time.', fr: 'Le tri et le regroupement de la liste, et ce avec quoi démarre une nouvelle session : réglé une fois plutôt qu’à chaque fois.' }
			},
			expect: [{ visible: 'page[sessions]' }],
			capture: 'sessions'
		},
		{
			id: 'execution',
			route: '/settings/execution',
			target: 'page[execution]',
			say: {
				title: { en: 'How much rope an agent gets', fr: 'La latitude laissée à un agent' },
				body: { en: 'Permission handling, auto-approval and the phrases that mean an agent has stopped early rather than finished.', fr: 'La gestion des permissions, l’approbation automatique et les formules qui signalent qu’un agent s’est arrêté en chemin plutôt que terminé.' }
			},
			expect: [{ visible: 'page[execution]' }],
			capture: 'execution'
		},
		{
			id: 'privacy',
			route: '/settings/privacy',
			target: 'page[privacy]',
			say: {
				title: { en: 'Secrets never reach the transcript', fr: 'Les secrets n’atteignent jamais la transcription' },
				body: { en: 'Tokens and keys are detected and replaced before anything is stored, and you can add patterns of your own.', fr: 'Les jetons et les clés sont détectés et remplacés avant tout enregistrement, et vous pouvez ajouter vos propres motifs.' }
			},
			expect: [{ visible: 'page[privacy]' }],
			capture: 'privacy'
		}
	]
});
