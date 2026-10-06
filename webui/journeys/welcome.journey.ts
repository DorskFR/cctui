import { defineJourney } from '@dorsk/journey';

/**
 * WHAT'S-NEW CONTRACT: shipping a notable feature means bumping `version` here
 * and rewriting the closing step. The done marker is keyed `id@version`, so a
 * bump marks the tour as new again on the Guides page for everyone who saw the
 * previous one; leaving the version alone means nobody is told about the new copy.
 *
 * No journey autostarts: a guide runs only when the user starts it from the
 * Guides page.
 *
 * Every target here must resolve on an instance with no session, no account and
 * no machine — this is the one tour that runs before the user has anything. Note
 * that `optional` cannot be used to soften that: the human actor polls for a
 * target without a timeout, so a missing one hangs instead of skipping.
 *
 * Routes are crossed by having the user click the real nav item. A step that
 * declares `route` instead makes the runtime interrupt with a "Go to another
 * page" card, which is why only the closing step carries one.
 */
const HOME = '/';

export default defineJourney({
	id: 'welcome',
	version: 5,
	title: { en: 'What cctui is', fr: 'Ce qu’est cctui' },
	description: {
		en: 'A walk through every screen, pointing at the real thing on each one.',
		fr: 'Un parcours de chaque écran, en désignant sur chacun l’élément réel.'
	},
	route: HOME,
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
					en: 'One control room for every coding agent you run. These tiles answer the first question before you touch anything — what is live, what is stuck, which machines are up.',
					fr: 'Un poste de contrôle pour chaque agent de code que vous exécutez. Ces tuiles répondent à la première question avant toute action : ce qui tourne, ce qui est bloqué, quelles machines sont en ligne.'
				}
			},
			expect: [{ visible: 'tiles' }],
			capture: 'overview'
		},
		{
			id: 'attention',
			target: 'tile[needs_input]',
			say: {
				title: { en: 'The only number that needs you', fr: 'Le seul chiffre qui vous réclame' },
				body: {
					en: 'An agent waiting on an answer stops making progress until you reply. That is why this count sits on the landing page rather than inside a list you have to go looking for.',
					fr: 'Un agent en attente de réponse cesse d’avancer jusqu’à ce que vous répondiez. D’où ce compteur sur la page d’accueil plutôt que dans une liste qu’il faudrait aller chercher.'
				}
			},
			capture: 'attention'
		},
		{
			id: 'to-sessions',
			target: 'nav[sessions]',
			do: { kind: 'click' },
			guide: 'next',
			say: {
				title: { en: 'The nav is the whole app', fr: 'La navigation, c’est toute l’application' },
				body: {
					en: 'Every page hangs off this rail. Sessions is where most of the work happens — open it.',
					fr: 'Toutes les pages partent de cette barre. C’est dans Sessions que se passe l’essentiel du travail — ouvrez-la.'
				}
			},
			capture: 'nav'
		},
		{
			id: 'sessions',
			target: { css: '[data-journey="session-list"], [data-journey="session-tiles"]' },
			say: {
				title: { en: 'Sessions — where the work happens', fr: 'Sessions — là où le travail se fait' },
				body: {
					en: 'Every run you have started, grouped by what it needs from you. The ones blocked on an answer rise to the top, so a long fleet still reads in one glance.',
					fr: 'Chaque exécution lancée, regroupée selon ce qu’elle attend de vous. Celles bloquées sur une réponse remontent en tête : une longue flotte se lit toujours d’un coup d’œil.'
				}
			},
			expect: [{ visible: { css: '[data-journey="session-list"], [data-journey="session-tiles"]' } }],
			capture: 'sessions'
		},
		{
			id: 'start',
			// A docked spawn panel replaces the New button.
			target: { css: '[data-journey="new"], [data-journey="spawn"]' },
			say: {
				title: { en: 'Starting one', fr: 'En démarrer une' },
				body: {
					en: 'This turns a prompt into a running agent: pick the machine, the folder and the account, and it works in the background whether or not you keep the tab open.',
					fr: 'Ceci transforme une instruction en agent actif : choisissez la machine, le dossier et le compte, et il travaille en arrière-plan, que vous gardiez l’onglet ouvert ou non.'
				}
			},
			expect: [{ visible: { css: '[data-journey="new"], [data-journey="spawn"]' } }],
			capture: 'start'
		},
		{
			id: 'to-settings',
			target: 'nav[settings]',
			do: { kind: 'click' },
			guide: 'next',
			say: {
				title: { en: 'Where does the quota come from?', fr: 'D’où vient le quota ?' },
				body: {
					en: 'An agent spends somebody’s tokens, on somebody’s machine. Both are set up in Settings — open it.',
					fr: 'Un agent dépense les jetons de quelqu’un, sur la machine de quelqu’un. Les deux se configurent dans Réglages — ouvrez-les.'
				}
			}
		},
		{
			id: 'to-accounts',
			when: { viewport: 'desktop' },
			target: 'settings-nav[accounts]',
			do: { kind: 'click' },
			guide: 'next',
			say: {
				title: { en: 'Whose tokens?', fr: 'Les jetons de qui ?' },
				body: {
					en: 'Open AI accounts to see whose.',
					fr: 'Ouvrez Comptes IA pour voir ceux de qui.'
				}
			}
		},
		{
			id: 'to-accounts-mobile',
			when: { viewport: 'mobile' },
			target: 'settings-tab[accounts]',
			do: { kind: 'click' },
			guide: 'next',
			say: {
				title: { en: 'Whose tokens?', fr: 'Les jetons de qui ?' },
				body: {
					en: 'Open AI accounts to see whose.',
					fr: 'Ouvrez Comptes IA pour voir ceux de qui.'
				}
			}
		},
		{
			id: 'accounts',
			target: 'page[accounts]',
			say: {
				title: { en: 'AI accounts — the credentials work runs on', fr: 'Comptes IA — les identifiants sur lesquels le travail tourne' },
				body: {
					en: 'Provider accounts can be grouped into pools, and a session can draw from a pool rather than one fixed account, so one hitting a rate limit steps aside instead of stalling the queue.',
					fr: 'Les comptes fournisseurs peuvent être regroupés en pools, et une session peut puiser dans un pool plutôt que dans un compte fixe : celui qui atteint sa limite s’efface au lieu de bloquer la file.'
				}
			},
			expect: [{ visible: 'page[accounts]' }],
			capture: 'accounts'
		},
		{
			id: 'to-users',
			when: { viewport: 'desktop' },
			target: 'settings-nav[users]',
			do: { kind: 'click' },
			guide: 'next',
			say: {
				title: { en: 'And where does it run?', fr: 'Et où cela s’exécute-t-il ?' },
				body: {
					en: 'Not here — cctui runs nothing itself. Open Users & keys to meet the machines that do.',
					fr: 'Pas ici — cctui n’exécute rien lui-même. Ouvrez Utilisateurs et clés pour rencontrer les machines qui le font.'
				}
			}
		},
		{
			id: 'to-users-mobile',
			when: { viewport: 'mobile' },
			target: 'settings-tab[users]',
			do: { kind: 'click' },
			guide: 'next',
			say: {
				title: { en: 'And where does it run?', fr: 'Et où cela s’exécute-t-il ?' },
				body: {
					en: 'Not here — cctui runs nothing itself. Open Users & keys to meet the machines that do.',
					fr: 'Pas ici — cctui n’exécute rien lui-même. Ouvrez Utilisateurs et clés pour rencontrer les machines qui le font.'
				}
			}
		},
		{
			id: 'users',
			target: 'page[users]',
			say: {
				title: { en: 'Users & keys — who may run work', fr: 'Utilisateurs et clés — qui peut exécuter le travail' },
				body: {
					en: 'Machines supply the compute, and they join the fleet from here with a single enrolment command. If a session cannot find anywhere to run, this is the page to open.',
					fr: 'Les machines fournissent la puissance de calcul et rejoignent la flotte ici, via une unique commande d’enrôlement. Si une session ne trouve nulle part où tourner, ouvrez cette page.'
				}
			},
			expect: [{ visible: 'page[users]' }],
			capture: 'access'
		},
		{
			id: 'settings',
			target: 'settings-goto',
			say: {
				title: { en: 'Everything else lives in Settings', fr: 'Tout le reste vit dans Réglages' },
				body: {
					en: 'How the app looks, how much rope an agent gets, what never leaves this machine — and the guides themselves, which is where we are going next.',
					fr: 'L’apparence de l’application, la latitude laissée aux agents, ce qui ne quitte jamais cette machine — et les guides eux-mêmes, où nous allons maintenant.'
				}
			},
			expect: [{ visible: 'settings-goto' }]
		},
		{
			id: 'guides',
			route: '/settings/guides',
			target: 'guide[sessions-list]',
			say: {
				title: { en: 'Now walk it for real', fr: 'Maintenant, parcourez-la pour de vrai' },
				body: {
					en: 'That is every screen. The rest of the guides run inside the app and point at the real controls in order — read the fleet, connect an account, enrol a machine, start a session, follow it. Start with this one.',
					fr: 'Voilà tous les écrans. Les autres guides se déroulent dans l’application et désignent les vrais contrôles, dans l’ordre : lire la flotte, connecter un compte, enrôler une machine, lancer une session, la suivre. Commencez par celui-ci.'
				}
			},
			expect: [{ visible: 'guide[sessions-list]' }],
			capture: 'guides'
		}
	]
});
