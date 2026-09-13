import { defineJourney } from '@dorsk/journey';

// A section header renders per group, so a bare path is ambiguous and throws.
// `nth` indexes the visible matches and exists only on the locator form.
const firstGroup = (name: string) => ({ css: `[data-journey="${name}"]`, nth: 0 }) as const;
const GROUP_SORT = firstGroup('group-sort');
const GROUP_HIDE = firstGroup('group-hide');
const VIEW = firstGroup('view');

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
				title: { en: 'Every session, grouped', fr: 'Chaque session, regroupée' },
				body: {
					en: 'Agents are grouped by what they need from you — the ones waiting on an answer rise to the top, so a long fleet still reads in one glance.',
					fr: 'Les agents sont regroupés selon ce qu’ils attendent de vous — ceux en attente de réponse remontent en tête, pour qu’une longue flotte se lise d’un seul coup d’œil.'
				}
			},
			expect: [{ visible: 'sections' }, { visible: 'search' }]
		},
		{
			id: 'list-fixture',
			qaOnly: true,
			target: 'section[blocked]',
			expect: [{ count: ['session', { min: 4 }] }, { visible: 'section[blocked]' }],
			capture: 'list'
		},
		{
			id: 'search',
			target: 'search',
			say: {
				title: { en: 'Narrow the list as you type', fr: 'Réduire la liste à mesure que vous tapez' },
				body: {
					en: 'Plain words match titles and prompts. Add a field — machine, label, status — to cut straight to a slice of the fleet instead of scrolling it.',
					fr: 'Les mots simples cherchent dans les titres et les prompts. Ajoutez un champ — machine, étiquette, statut — pour atteindre directement une partie de la flotte au lieu de la parcourir.'
				}
			},
			expect: [{ visible: 'search' }],
			capture: 'search'
		},
		{
			id: 'sections',
			target: 'sections/toggle',
			do: { kind: 'click' },
			say: {
				title: { en: 'Choose which groups you see', fr: 'Choisir les groupes affichés' },
				body: {
					en: 'Each group is an independent switch. Turning off the ones you are not working in is what keeps the list short once the fleet grows.',
					fr: 'Chaque groupe est un interrupteur indépendant. Désactiver ceux sur lesquels vous ne travaillez pas est ce qui garde la liste courte quand la flotte grandit.'
				}
			},
			expect: [{ visible: 'sections/option[live]' }, { visible: 'sections/option[archived]' }]
		},
		{
			id: 'starred',
			target: 'sections/option[starred]',
			say: {
				title: { en: 'Keep the ones that matter in reach', fr: 'Garder à portée celles qui comptent' },
				body: {
					en: 'Starred sessions get their own group above everything else — the fastest way to pin the two or three runs you actually care about today.',
					fr: 'Les sessions favorites forment leur propre groupe, au-dessus du reste — le moyen le plus rapide d’épingler les deux ou trois exécutions qui comptent aujourd’hui.'
				}
			},
			expect: [{ visible: 'sections/option[starred]' }],
			capture: 'sections'
		},
		{
			id: 'options',
			target: 'options',
			do: { kind: 'click' },
			say: {
				title: { en: 'Change how the list is drawn', fr: 'Changer la façon dont la liste est dessinée' },
				body: {
					en: 'The secondary controls live behind this button so the search bar keeps its width. Everything here is a view setting — it never touches the sessions themselves.',
					fr: 'Les contrôles secondaires vivent derrière ce bouton pour que la barre de recherche garde sa largeur. Tout ici est un réglage d’affichage — rien n’agit sur les sessions elles-mêmes.'
				}
			},
			expect: [{ visible: 'display-options' }]
		},
		{
			id: 'grouping',
			target: 'display-options/dimension[group]',
			say: {
				title: { en: 'Group by whatever you are debugging', fr: 'Regrouper selon ce que vous déboguez' },
				body: {
					en: 'Group by machine when you suspect one box, by project when you are context-switching. Colour-by tints the cards on a second dimension, so you can read both at once.',
					fr: 'Regroupez par machine quand vous soupçonnez une machine, par projet quand vous jonglez entre contextes. La couleur teinte les cartes sur une seconde dimension, pour lire les deux à la fois.'
				}
			},
			expect: [{ visible: 'display-options/dimension[color]' }],
			capture: 'options'
		},
		{
			id: 'view',
			target: VIEW,
			say: {
				title: { en: 'Dense rows or roomy cards', fr: 'Lignes denses ou cartes aérées' },
				body: {
					en: 'Rows fit more of the fleet on screen; cards give each session room for its prompt and its latest activity. The choice sticks between visits.',
					fr: 'Les lignes affichent plus de la flotte à l’écran ; les cartes laissent à chaque session la place de son prompt et de sa dernière activité. Le choix est conservé d’une visite à l’autre.'
				}
			},
			expect: [{ visible: VIEW }]
		},
		{
			id: 'group-sort',
			optional: true,
			target: GROUP_SORT,
			say: {
				title: { en: 'Each group sorts on its own', fr: 'Chaque groupe se trie séparément' },
				body: {
					en: 'Sort by last activity to see what just moved, by name when you are looking for one you already know. The order applies to every group at once.',
					fr: 'Triez par dernière activité pour voir ce qui vient de bouger, par nom quand vous cherchez une session que vous connaissez déjà. L’ordre s’applique à tous les groupes.'
				}
			},
			expect: [{ visible: GROUP_SORT }]
		},
		{
			id: 'group-actions',
			optional: true,
			target: GROUP_HIDE,
			say: {
				title: { en: 'Collapse or clear a whole group', fr: 'Replier ou vider un groupe entier' },
				body: {
					en: 'The eye folds a group away without losing it. Beside it, archive retires every session in that group in one move — how a finished batch leaves the list for good.',
					fr: 'L’œil replie un groupe sans le perdre. À côté, l’archivage retire toutes les sessions du groupe d’un seul geste — c’est ainsi qu’un lot terminé quitte la liste définitivement.'
				}
			},
			expect: [{ visible: GROUP_HIDE }],
			capture: 'group'
		}
	]
});
