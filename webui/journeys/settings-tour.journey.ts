import { defineJourney } from '@dorsk/journey';

/**
 * Areas are reached by having the user click the area switcher: `settings-goto`
 * is on the wide list and on the narrow tab strip, exactly one of which is ever
 * visible, so one step serves both widths. Each content step still declares its
 * `route` — a no-op once the click arrived, and the fallback if it did not.
 */
const INTERFACE = '/settings/interface';
const SESSIONS = '/settings/sessions';
const hop = (page: string) => ({
	css: `.toc [data-journey="settings-nav"][data-journey-key="${page}"], .tabs [data-journey="settings-tab"][data-journey-key="${page}"]`
});

export default defineJourney({
	id: 'settings-tour',
	title: { en: 'Tune how agents behave', fr: 'Régler le comportement des agents' },
	description: { en: 'Settings cover the look of the app, how sessions run, and what never leaves the machine.', fr: 'Les réglages couvrent l’apparence de l’application, l’exécution des sessions et ce qui ne quitte jamais la machine.' },
	route: INTERFACE,
	variants: { viewport: ['desktop', 'mobile'], theme: ['dark'] },
	level: 'checked',
	steps: [
		{
			id: 'theme',
			route: INTERFACE,
			target: 'theme',
			say: {
				title: { en: 'Start with the one you will look at all day', fr: 'Commencez par celui que vous regarderez toute la journée' },
				body: { en: 'Pick a theme, or let it follow the system so it turns dark when your desktop does. Every setting on these pages saves itself the moment you change it — there is no Save button anywhere in Settings.', fr: 'Choisissez un thème, ou laissez-le suivre le système pour qu’il passe en sombre avec votre bureau. Chaque réglage de ces pages s’enregistre dès que vous le changez — il n’y a aucun bouton Enregistrer dans les Réglages.' }
			},
			expect: [{ visible: 'theme' }],
			capture: 'theme'
		},
		{
			id: 'language',
			target: 'language',
			say: {
				title: { en: 'The interface language', fr: 'La langue de l’interface' },
				body: { en: 'Switching it re-draws the app immediately, including a guide you happen to have open — this one will follow you into French mid-sentence if you try it.', fr: 'En changer redessine l’application immédiatement, y compris un guide ouvert — celui-ci vous suivra en anglais au milieu d’une phrase si vous essayez.' }
			},
			expect: [{ visible: 'language' }],
			capture: 'language'
		},
		{
			id: 'to-sessions',
			target: hop('sessions'),
			do: { kind: 'click' },
			guide: 'next',
			say: {
				title: { en: 'Settings come in a few areas', fr: 'Les réglages tiennent en quelques rubriques' },
				body: { en: 'The highlighted switcher is how you move between them — a list beside the page on a wide screen, a strip of tabs along the top on a narrow one. Pick “Sessions” from it, not from the app’s own nav.', fr: 'Le sélecteur en surbrillance permet de passer de l’une à l’autre — une liste à côté de la page sur grand écran, une rangée d’onglets en haut sur écran étroit. Choisissez « Sessions » dedans, pas dans la navigation de l’application.' }
			}
		},
		{
			id: 'sessions',
			route: SESSIONS,
			target: 'setting[auto-resume]',
			say: {
				title: { en: 'Defaults for every run', fr: 'Les valeurs par défaut de chaque exécution' },
				body: { en: 'This area decides how sessions launch and how a conversation behaves before you touch either — profiles, follow-ups, archiving, and this one: whether a dropped connection silently resumes the run or leaves it for you to notice.', fr: 'Cette rubrique décide du lancement des sessions et du comportement d’une conversation avant toute intervention — profils, relances, archivage, et ceci : si une connexion perdue reprend l’exécution en silence ou vous laisse le constater.' }
			},
			expect: [{ visible: 'setting[auto-resume]' }],
			capture: 'sessions'
		},
		{
			id: 'harness',
			route: SESSIONS,
			target: 'harness-mode',
			say: {
				title: { en: 'The single most consequential setting', fr: 'Le réglage le plus lourd de conséquences' },
				body: { en: 'How much rope an agent gets: whether it asks before each action, or edits and runs commands on its own. Loosen it and agents finish far more without you — and can do far more damage unattended. Decide it deliberately, per machine you trust.', fr: 'La latitude laissée à un agent : demander avant chaque action, ou modifier et exécuter des commandes seul. En l’assouplissant, les agents terminent bien plus sans vous — et peuvent faire bien plus de dégâts sans surveillance. Décidez-en délibérément, machine par machine.' }
			},
			expect: [{ visible: 'harness-mode' }],
			capture: 'execution'
		},
		{
			id: 'patterns',
			route: SESSIONS,
			target: 'redact-patterns',
			say: {
				title: { en: 'Secrets never reach the transcript', fr: 'Les secrets n’atteignent jamais la transcription' },
				body: { en: 'Anything matching these patterns is redacted before a transcript is stored, so a key pasted into a prompt does not end up in the history. Add the shapes only you would recognise — your own token prefixes, internal hostnames.', fr: 'Tout ce qui correspond à ces motifs est masqué avant l’enregistrement d’une transcription : une clé collée dans une instruction ne finit pas dans l’historique. Ajoutez les formes que vous seul reconnaissez — vos préfixes de jetons, vos noms d’hôtes internes.' }
			},
			expect: [{ visible: 'redact-patterns' }],
			capture: 'privacy'
		},
		{
			id: 'to-guides',
			target: hop('interface'),
			do: { kind: 'click' },
			guide: 'next',
			say: {
				title: { en: 'Back where you started', fr: 'Retour au point de départ' },
				body: { en: 'Pick “Interface” from the highlighted switcher — the guides live at the bottom of it.', fr: 'Choisissez « Interface » dans le sélecteur en surbrillance — les guides sont tout en bas.' }
			}
		},
		{
			id: 'guides',
			route: INTERFACE,
			target: 'guide[welcome]',
			say: {
				title: { en: 'Every guide lives here', fr: 'Tous les guides vivent ici' },
				body: { en: 'This page is the curriculum: what each guide teaches, what it is worth, and what is still locked. Replay any of them whenever you like — nothing here changes your instance. There is more than what we walked; accounts, notifications, plugins, macros and the admin Instance area are all worth a look once you are running.', fr: 'Cette page est le programme : ce que chaque guide enseigne, sa valeur, et ce qui reste verrouillé. Rejouez-en un quand vous voulez — rien ici ne modifie votre instance. Il y a plus que ce que nous avons parcouru ; comptes, notifications, extensions, macros et la rubrique d’administration Instance méritent un coup d’œil une fois lancé.' }
			},
			expect: [{ visible: 'guide[welcome]' }],
			capture: 'guides'
		}
	]
});
