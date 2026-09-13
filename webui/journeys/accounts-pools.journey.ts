import { defineJourney } from '@dorsk/journey';

const LOOSE = 'acme-research';
const BOARD = '/accounts';
// Positional: a real board holds any number of cards, so a path segment would
// resolve ambiguously and fail.
const ANY_ACCOUNT = { css: '[data-journey="account"]', nth: 0 } as const;

export default defineJourney({
	id: 'accounts-pools',
	title: { en: 'Connect a provider account', fr: 'Connecter un compte fournisseur' },
	description: { en: 'Accounts are the credentials work runs on. Add one so a session has something to run with.', fr: 'Les comptes sont les identifiants sur lesquels le travail s’exécute. Ajoutez-en un pour qu’une session ait de quoi tourner.' },
	route: BOARD,
	variants: { viewport: ['desktop', 'mobile'], theme: ['dark'] },
	level: 'checked',
	steps: [
		{
			id: 'board',
			route: BOARD,
			target: 'accounts',
			say: {
				title: { en: 'Every account you can run on', fr: 'Tous les comptes sur lesquels vous pouvez travailler' },
				body: { en: 'Accounts are the provider credentials your agents run on. This board is empty until you add one, and nothing can be launched before it has one.', fr: 'Les comptes sont les identifiants fournisseur sur lesquels vos agents s’exécutent. Ce tableau reste vide jusqu’à ce que vous en ajoutiez un, et rien ne peut être lancé avant.' }
			},
			expect: [{ visible: 'accounts' }],
			capture: 'board'
		},
		{
			id: 'card',
			route: BOARD,
			optional: true,
			target: ANY_ACCOUNT,
			say: {
				title: { en: 'What a card tells you', fr: 'Ce que dit une carte' },
				body: { en: 'Each card is one credential: who owns it, which providers it carries, and how much of its budget is already spent. The grip on its edge is how it joins a pool.', fr: 'Chaque carte représente un identifiant : son propriétaire, les fournisseurs qu’il porte et la part de budget déjà consommée. La poignée sur son bord permet de le rattacher à un pool.' }
			}
		},
		{
			id: 'pool',
			qaOnly: true,
			target: 'pool[production]',
			say: {
				title: { en: 'A pool is a set of interchangeable accounts', fr: 'Un pool est un ensemble de comptes interchangeables' },
				body: { en: 'Launches aimed at the pool elect a member by headroom, so a spent weekly budget on one account does not stop the work.', fr: 'Les lancements visant le pool élisent un membre selon sa marge restante : un budget hebdomadaire épuisé sur un compte n’arrête pas le travail.' }
			},
			expect: [{ visible: 'pool[production]' }],
			capture: 'pool'
		},
		{
			id: 'handle',
			qaOnly: true,
			target: `account[${LOOSE}]/drag-handle`,
			say: {
				title: { en: 'Drag an account into a pool', fr: 'Glisser un compte dans un pool' },
				body: { en: 'The grip on a card lifts it; dropping it on a pool adds it to the membership.', fr: 'La poignée d’une carte la soulève ; la déposer sur un pool l’ajoute aux membres.' }
			},
			expect: [{ visible: `account[${LOOSE}]/drag-handle` }],
			capture: 'handle'
		},
		{
			id: 'menu',
			qaOnly: true,
			target: `account[${LOOSE}]/account-menu`,
			do: { kind: 'click' },
			say: {
				title: { en: 'Or add it from the card menu', fr: 'Ou l’ajouter depuis le menu de la carte' },
				body: { en: 'The same membership change without a mouse drag, which is also how it is done on a phone.', fr: 'Le même changement d’appartenance sans glisser-déposer, et c’est aussi la méthode sur téléphone.' }
			},
			capture: 'menu'
		},
		{
			id: 'pools',
			route: BOARD,
			target: 'new-pool',
			say: {
				title: { en: 'Group accounts once you have several', fr: 'Regrouper les comptes quand vous en avez plusieurs' },
				body: { en: 'A pool is a set of interchangeable accounts. Aim a session at the pool instead of one account and it picks whichever member has budget left, so a weekly limit on one does not halt your work.', fr: 'Un pool est un ensemble de comptes interchangeables. Visez le pool plutôt qu’un compte précis : il choisit le membre qui a encore du budget, et une limite hebdomadaire n’arrête pas votre travail.' }
			},
			expect: [{ visible: 'new-pool' }]
		},
		// Must stay last: the click opens a modal over the board, hiding every
		// capture above it.
		{
			id: 'add',
			route: BOARD,
			target: 'new-account',
			do: { kind: 'click' },
			say: {
				title: { en: 'Add your first account', fr: 'Ajoutez votre premier compte' },
				body: { en: 'Add the credential your agents will run on. Once it is saved you can launch a session; pools are for when you have more than one.', fr: 'Ajoutez l’identifiant sur lequel vos agents s’exécuteront. Une fois enregistré, vous pouvez lancer une session ; les pools servent quand vous en avez plusieurs.' }
			}
		},
		{
			id: 'done',
			route: '/settings/guides',
			target: 'page[guides]',
			say: {
				title: { en: 'Your agents have something to run on', fr: 'Vos agents ont de quoi s’exécuter' },
				body: { en: 'You know where credentials live, what a card is telling you, and why pools exist. Next, bring a machine into the fleet so there is somewhere for the work to happen.', fr: 'Vous savez où vivent les identifiants, ce que dit une carte et à quoi servent les pools. Ensuite, enrôlez une machine pour que le travail ait un endroit où s’exécuter.' }
			},
			expect: [{ visible: 'page[guides]' }]
		}
	]
});
