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
				body: { en: 'Theme, text size, language and where the navigation sits. Every one of these is remembered per account, so the app looks the same on your phone as on your desk.', fr: 'Thème, taille du texte, langue et position de la navigation. Chacun de ces réglages est mémorisé par compte : l’application est identique sur votre téléphone et sur votre bureau.' }
			},
			expect: [{ visible: 'page[appearance]' }],
			capture: 'appearance'
		},
		{
			id: 'theme',
			route: '/settings/appearance',
			target: 'page[appearance]/theme',
			say: {
				title: { en: 'Pick a theme, or let it follow the system', fr: 'Choisir un thème, ou suivre le système' },
				body: { en: 'Auto remembers one light and one dark theme and swaps between them as your system does. Pick a specific theme instead and it stays put at every hour of the day.', fr: 'Le mode auto mémorise un thème clair et un thème sombre et bascule comme votre système. Choisissez un thème précis et il ne bougera plus, quelle que soit l’heure.' }
			},
			expect: [{ visible: 'page[appearance]/theme' }],
			capture: 'theme'
		},
		{
			id: 'language',
			route: '/settings/appearance',
			target: 'page[appearance]/language',
			say: {
				title: { en: 'The interface language', fr: 'La langue de l’interface' },
				body: { en: 'This changes the app, not your agents — what you write in a prompt is still up to you. Left on automatic it follows your browser.', fr: 'Ceci change l’application, pas vos agents : ce que vous écrivez dans une instruction reste votre choix. En automatique, la langue suit celle du navigateur.' }
			},
			expect: [{ visible: 'page[appearance]/language' }]
		},
		{
			id: 'sessions',
			route: '/settings/sessions',
			target: 'page[sessions]',
			say: {
				title: { en: 'Defaults for every run', fr: 'Les réglages par défaut de chaque run' },
				body: { en: 'How the list sorts and groups, and what a new session starts with, so you set it once instead of every time. The conversation options here also decide how much of a transcript you see at a glance.', fr: 'Le tri et le regroupement de la liste, et ce avec quoi démarre une nouvelle session : réglé une fois plutôt qu’à chaque fois. Les options de conversation décident aussi de ce que vous voyez d’une transcription d’un coup d’œil.' }
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
			id: 'harness',
			route: '/settings/execution',
			target: 'page[execution]/harness-mode',
			say: {
				title: { en: 'The single most consequential setting', fr: 'Le réglage le plus lourd de conséquences' },
				body: { en: 'This decides whether an agent stops to ask before it acts, or proceeds on its own. Loosen it for throwaway work in a scratch folder; keep it tight anywhere a wrong edit would cost you.', fr: 'Il décide si un agent s’arrête pour demander avant d’agir, ou s’il continue seul. Relâchez-le pour du travail jetable dans un dossier de test ; gardez-le strict partout où une mauvaise modification coûterait cher.' }
			},
			expect: [{ visible: 'page[execution]/harness-mode' }]
		},
		{
			id: 'privacy',
			route: '/settings/privacy',
			target: 'page[privacy]',
			say: {
				title: { en: 'Secrets never reach the transcript', fr: 'Les secrets n’atteignent jamais la transcription' },
				body: { en: 'Tokens and keys are detected and replaced before anything is stored, so a leaked credential does not end up sitting in your history.', fr: 'Les jetons et les clés sont détectés et remplacés avant tout enregistrement : un identifiant qui fuit ne reste pas dans votre historique.' }
			},
			expect: [{ visible: 'page[privacy]' }],
			capture: 'privacy'
		},
		{
			id: 'patterns',
			route: '/settings/privacy',
			target: 'page[privacy]/redact-patterns',
			say: {
				title: { en: 'Add the secrets only you can recognise', fr: 'Ajoutez les secrets que vous seul reconnaissez' },
				body: { en: 'The built-in detectors know the common credential shapes. Your own internal ticket or key formats are not among them — add a pattern per line and they are redacted too.', fr: 'Les détecteurs intégrés connaissent les formats d’identifiants courants. Vos formats internes de tickets ou de clés n’en font pas partie : ajoutez un motif par ligne et ils seront masqués aussi.' }
			},
			expect: [{ visible: 'page[privacy]/redact-patterns' }]
		}
	]
});
