import { defineJourney } from '@dorsk/journey';

const BOARD = '/accounts';
// Every step anchors on board furniture rather than on a card: a user who has no
// account yet is exactly who this guide is for, and `optional` cannot rescue a
// missing target once a human is driving.
const CLOSE = { role: 'button', name: 'Cancel' } as const;

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
				body: { en: 'An account is one provider credential — the thing an agent bills its tokens to. This board is the whole list, and until it holds one, nothing can be launched.', fr: 'Un compte est un identifiant fournisseur — ce sur quoi un agent facture ses jetons. Ce tableau est la liste complète, et tant qu’il est vide, rien ne peut être lancé.' }
			},
			expect: [{ visible: 'accounts' }],
			capture: 'board'
		},
		{
			id: 'anatomy',
			target: 'accounts',
			say: {
				title: { en: 'What each card tells you', fr: 'Ce que dit chaque carte' },
				body: { en: 'A card carries its name and emoji, who owns it, which providers and models it can reach, and how much of its budget is already spent. The grip on its left edge is how it joins a pool.', fr: 'Une carte porte son nom et son emoji, son propriétaire, les fournisseurs et modèles qu’elle atteint, et la part de budget déjà consommée. La poignée sur son bord gauche permet de la rattacher à un pool.' }
			},
			capture: 'anatomy'
		},
		{
			id: 'pools',
			target: 'new-pool',
			say: {
				title: { en: 'A pool is a set of interchangeable accounts', fr: 'Un pool est un ensemble de comptes interchangeables' },
				body: { en: 'Aim a session at a pool instead of one account and it elects whichever member has the most budget left, so a weekly limit reached on one account does not stop your work. Drag a card by its grip onto a pool to add it, or use the card’s own menu — the same change without a mouse, which is how it is done on a phone.', fr: 'Visez un pool plutôt qu’un compte précis : il élit le membre qui a le plus de budget restant, et une limite hebdomadaire atteinte sur un compte n’arrête pas votre travail. Glissez une carte par sa poignée sur un pool pour l’ajouter, ou passez par le menu de la carte — le même changement sans souris, et c’est la méthode sur téléphone.' }
			},
			expect: [{ visible: 'new-pool' }],
			capture: 'pools'
		},
		{
			id: 'add',
			target: 'new-account',
			do: { kind: 'click' },
			say: {
				title: { en: 'Add your first account', fr: 'Ajoutez votre premier compte' },
				body: { en: 'Open the dialog. You will name the credential, pick its provider and paste the key — nothing leaves this instance, and the key is encrypted before it is stored.', fr: 'Ouvrez la boîte de dialogue. Vous nommerez l’identifiant, choisirez son fournisseur et collerez la clé — rien ne quitte cette instance, et la clé est chiffrée avant d’être stockée.' }
			},
			capture: 'add'
		},
		{
			id: 'close',
			target: CLOSE,
			do: { kind: 'click' },
			say: {
				title: { en: 'Close it for now', fr: 'Fermez-la pour l’instant' },
				body: { en: 'Nothing is saved until you submit, so dismissing it changes nothing. Come back with a real key and the board will have its first card — then a session has something to run on.', fr: 'Rien n’est enregistré avant validation : fermer ne change rien. Revenez avec une vraie clé et le tableau aura sa première carte — une session aura alors de quoi tourner.' }
			},
			expect: [{ visible: 'new-account' }],
			capture: 'closed'
		}
	]
});
