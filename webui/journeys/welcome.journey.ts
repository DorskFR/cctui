import { defineJourney } from '@dorsk/journey';

/**
 * WHAT'S-NEW CONTRACT: shipping a notable feature means bumping `version` here
 * and rewriting the closing step. The done marker is keyed `id@version`, so a
 * bump re-shows the tour exactly once to everyone who saw the previous one;
 * leaving the version alone means nobody ever sees the new copy.
 *
 * Every target here must resolve on an instance with no session, no account and
 * no machine — this is the one tour that runs before the user has anything.
 * Anything data-dependent is `optional`, which skips it rather than failing the
 * run.
 */
const HOME = '/';
const SESSIONS = '/sessions';

export default defineJourney({
	id: 'welcome',
	version: 3,
	title: { en: 'What cctui is', fr: 'Ce qu’est cctui' },
	description: {
		en: 'A walk through every screen, pointing at the real thing on each one.',
		fr: 'Un parcours de chaque écran, en désignant sur chacun l’élément réel.'
	},
	route: HOME,
	autostart: { route: HOME, once: true },
	level: 'checked',
	variants: { viewport: ['desktop', 'mobile'], theme: ['dark'] },
	steps: [
		{
			id: 'overview',
			route: HOME,
			target: 'tiles',
			say: {
				title: { en: 'Start here: is the fleet healthy?', fr: 'Commencez ici : la flotte est-elle en bonne santé ?' },
				body: {
					en: 'One control room for every Claude Code session you run. These tiles answer the first question before you touch anything — what is live, what is stuck, which machines are up.',
					fr: 'Un poste de contrôle pour chaque session Claude Code. Ces tuiles répondent à la première question avant toute action : ce qui tourne, ce qui est bloqué, quelles machines sont en ligne.'
				}
			},
			expect: [{ visible: 'tiles' }],
			capture: 'overview'
		},
		{
			id: 'attention',
			route: HOME,
			optional: true,
			target: 'tile[needs_input]',
			say: {
				title: { en: 'The only number that needs you', fr: 'Le seul chiffre qui vous réclame' },
				body: {
					en: 'An agent waiting on an answer stops making progress until you reply. That is why this count sits on the landing page rather than inside a list you have to go looking for.',
					fr: 'Un agent en attente de réponse cesse d’avancer jusqu’à ce que vous répondiez. D’où ce compteur sur la page d’accueil plutôt que dans une liste qu’il faudrait aller chercher.'
				}
			}
		},
		{
			id: 'sessions',
			route: SESSIONS,
			target: 'session-list',
			say: {
				title: { en: 'Sessions — where the work happens', fr: 'Sessions — là où le travail se fait' },
				body: {
					en: 'Every run you have started, grouped by what it needs from you. The ones blocked on an answer rise to the top, so a long fleet still reads in one glance.',
					fr: 'Chaque exécution lancée, regroupée selon ce qu’elle attend de vous. Celles bloquées sur une réponse remontent en tête : une longue flotte se lit toujours d’un coup d’œil.'
				}
			},
			expect: [{ visible: 'session-list' }],
			capture: 'sessions'
		},
		{
			id: 'start',
			route: SESSIONS,
			target: 'new',
			say: {
				title: { en: 'Starting one', fr: 'En démarrer une' },
				body: {
					en: 'Spawn turns a prompt into a running agent: pick the machine, the repository and the account pool, and it works in the background whether or not you keep the tab open.',
					fr: 'Spawn transforme une instruction en agent actif : choisissez la machine, le dépôt et le pool de comptes, et il travaille en arrière-plan, que vous gardiez l’onglet ouvert ou non.'
				}
			},
			expect: [{ visible: 'new' }]
		},
		{
			id: 'accounts',
			route: '/accounts',
			target: 'accounts',
			say: {
				title: { en: 'Accounts — where the quota comes from', fr: 'Comptes — d’où vient le quota' },
				body: {
					en: 'Provider accounts are grouped into pools, and a session draws from a pool rather than a fixed account, so one hitting a rate limit steps aside instead of stalling the queue.',
					fr: 'Les comptes fournisseurs sont regroupés en pools, et une session puise dans un pool plutôt que dans un compte fixe : celui qui atteint sa limite s’efface au lieu de bloquer la file.'
				}
			},
			expect: [{ visible: 'accounts' }],
			capture: 'accounts'
		},
		{
			id: 'access',
			route: '/access',
			target: 'enroll',
			say: {
				title: { en: 'Access — what may run work', fr: 'Accès — ce qui peut exécuter le travail' },
				body: {
					en: 'Machines supply the compute, and they join the fleet from here with a single enrolment command. If a session cannot find anywhere to run, this is the page to open.',
					fr: 'Les machines fournissent la puissance de calcul et rejoignent la flotte ici, via une unique commande d’enrôlement. Si une session ne trouve nulle part où tourner, ouvrez cette page.'
				}
			},
			expect: [{ visible: 'enroll' }],
			capture: 'access'
		},
		{
			id: 'usage',
			route: HOME,
			target: 'analytics',
			say: {
				title: { en: 'Usage — what the fleet costs', fr: 'Usage — ce que coûte la flotte' },
				body: {
					en: 'Tokens by hour, day, week and month, split by model and by account. An expensive habit shows up here well before it shows up on a bill.',
					fr: 'Les tokens par heure, jour, semaine et mois, ventilés par modèle et par compte. Une habitude coûteuse apparaît ici bien avant de figurer sur une facture.'
				}
			},
			expect: [{ visible: 'analytics' }]
		},
		{
			id: 'guides',
			route: '/settings/guides',
			target: 'page[guides]',
			say: {
				title: { en: 'Now walk it for real', fr: 'Maintenant, parcourez-la pour de vrai' },
				body: {
					en: 'That is every screen. The remaining guides run inside the app, pointing at the real controls in order — connect an account, enrol a machine, start a session, follow it. They all live on this page.',
					fr: 'Voilà tous les écrans. Les guides restants se déroulent dans l’application et désignent les vrais contrôles, dans l’ordre : connecter un compte, enrôler une machine, lancer une session, la suivre. Ils sont tous sur cette page.'
				}
			},
			expect: [{ visible: 'page[guides]' }],
			capture: 'guides'
		}
	]
});
