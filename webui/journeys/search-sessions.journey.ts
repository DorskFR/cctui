import { defineJourney } from '@dorsk/journey';

// Tsumikit's FilterSearchBar forwards no attributes to its input, so the book's
// fill steps address it by accessible name; public steps anchor on the cctui
// wrapper instead.
const BOX = { label: 'Search sessions', within: 'search' } as const;
const FIND_BOX = { label: 'Search in conversation', within: 'conversation-search' } as const;
const SESSIONS = '/sessions';
// Cards repeat, so a bare path matches every one of them and throws.
const FIRST_SESSION_TITLE = {
	css: '[data-journey="session"] [data-journey="title"]',
	nth: 0
} as const;

export default defineJourney({
	id: 'search-sessions',
	title: { en: 'Find anything across sessions', fr: 'Retrouver n’importe quoi dans vos sessions' },
	description: { en: 'Search reads every transcript, not just the titles, and narrows by machine, label or status.', fr: 'La recherche lit toutes les transcriptions, pas seulement les titres, et filtre par machine, libellé ou statut.' },
	route: SESSIONS,
	variants: { viewport: ['desktop', 'mobile'], theme: ['dark'] },
	level: 'checked',
	steps: [
		{
			id: 'box',
			route: SESSIONS,
			target: 'search',
			say: {
				title: { en: 'One box over every session', fr: 'Une seule boîte pour toutes vos sessions' },
				body: { en: 'This searches what the agents actually said and did, not just the names you gave the runs. A session you never labelled is still findable by what it touched.', fr: 'Elle cherche dans ce que les agents ont réellement dit et fait, pas seulement dans les noms que vous avez donnés. Une session jamais libellée reste trouvable par ce qu’elle a touché.' }
			},
			expect: [{ visible: 'search' }],
			capture: 'box'
		},
		{
			id: 'start',
			qaOnly: true,
			route: SESSIONS,
			target: 'section[blocked]',
			say: {
				title: { en: 'Start from the whole list', fr: 'Partir de la liste entière' },
				body: { en: 'Every session you have run is searchable, live ones and finished ones alike.', fr: 'Toutes vos sessions sont consultables, en cours comme terminées.' }
			},
			expect: [{ count: ['session', { min: 4 }] }],
			capture: 'before'
		},
		{
			id: 'free-text',
			qaOnly: true,
			target: BOX,
			do: { kind: 'fill', value: 'pagination' },
			say: {
				title: { en: 'Search the transcripts, not just the titles', fr: 'Chercher dans les transcriptions, pas seulement les titres' },
				body: { en: 'A plain word is matched against what the agents actually said and did, so you can find a run by what it touched.', fr: 'Un simple mot est comparé à ce que les agents ont dit et fait : vous retrouvez un run par ce qu’il a touché.' }
			},
			expect: [{ count: ['session', { equals: 1 }] }],
			capture: 'text'
		},
		{
			id: 'facet',
			qaOnly: true,
			target: BOX,
			do: { kind: 'fill', value: 'label:backend' },
			say: {
				title: { en: 'Narrow by label, machine or status', fr: 'Filtrer par libellé, machine ou statut' },
				body: { en: 'Typed filters like label:, machine: and status: combine with the free text to cut a large fleet down fast.', fr: 'Les filtres typés comme label:, machine: et status: se combinent au texte libre pour réduire vite une grande flotte.' }
			},
			expect: [{ count: ['session', { min: 2 }] }],
			capture: 'facet'
		},
		{
			id: 'facets',
			route: SESSIONS,
			target: 'search',
			say: {
				title: { en: 'Narrow it with a typed filter', fr: 'Affiner avec un filtre typé' },
				body: { en: 'Prefixes like label:, machine: and status: turn a word into a condition. Type one and the box offers the values it knows, so you need not remember them.', fr: 'Des préfixes comme label:, machine: et status: transforment un mot en condition. Tapez-en un et la boîte propose les valeurs qu’elle connaît : inutile de les mémoriser.' }
			},
			expect: [{ visible: 'search' }]
		},
		{
			id: 'combine',
			route: SESSIONS,
			target: 'search',
			say: {
				title: { en: 'Stack them to answer a real question', fr: 'Les combiner pour répondre à une vraie question' },
				body: { en: 'Filters and free text apply together: status: with a word finds the run that failed on the thing you remember. Clearing the box brings the whole fleet back.', fr: 'Filtres et texte libre s’appliquent ensemble : status: plus un mot retrouve le run qui a échoué sur ce dont vous vous souvenez. Videz la boîte pour retrouver toute la flotte.' }
			},
			expect: [{ visible: 'search' }]
		},
		{
			id: 'open-conversation',
			qaOnly: true,
			route: SESSIONS,
			target: FIRST_SESSION_TITLE,
			do: { kind: 'click' },
			say: {
				title: { en: 'Then search inside one', fr: 'Puis chercher dans une seule' },
				body: { en: 'The same box exists inside a conversation, scoped to that one transcript.', fr: 'La même boîte existe dans une conversation, limitée à cette seule transcription.' }
			},
			expect: [{ visible: 'conversation' }]
		},
		{
			id: 'open-find',
			qaOnly: true,
			target: 'conversation/header/find',
			do: { kind: 'click' },
			say: {
				title: { en: 'Open find in conversation', fr: 'Ouvrir la recherche dans la conversation' },
				body: { en: 'The bar opens under the toolbar, focused and ready.', fr: 'La barre s’ouvre sous la barre d’outils, prête à recevoir le curseur.' }
			},
			expect: [{ visible: 'conversation-search' }]
		},
		{
			id: 'find-text',
			qaOnly: true,
			target: FIND_BOX,
			do: { kind: 'fill', value: 'the' },
			say: {
				title: { en: 'The count comes from the server', fr: 'Le compte vient du serveur' },
				body: { en: 'Matches older than the loaded page are counted too, so the total is the whole transcript, not the part on screen.', fr: 'Les résultats plus anciens que la page chargée sont comptés aussi : le total couvre toute la transcription, pas seulement l’écran.' }
			},
			expect: [{ visible: 'conversation-search/hit-next' }]
		},
		{
			id: 'find-step',
			qaOnly: true,
			target: 'conversation-search/hit-next',
			do: { kind: 'click' },
			say: {
				title: { en: 'Step to a match', fr: 'Aller à un résultat' },
				body: { en: '↓ jumps to the next match, paging older history in when the match is older than what is loaded.', fr: '↓ saute au résultat suivant et charge l’historique plus ancien si le résultat s’y trouve.' }
			},
			expect: [{ visible: 'conversation' }]
		},
		{
			id: 'find-close',
			qaOnly: true,
			target: 'conversation-search/find-close',
			do: { kind: 'click' },
			say: {
				title: { en: 'Closing leaves the conversation open', fr: 'Fermer laisse la conversation ouverte' },
				body: { en: 'The bar closes and the highlight goes with it; the conversation itself stays where you were reading.', fr: 'La barre se ferme avec la surbrillance ; la conversation reste là où vous lisiez.' }
			},
			expect: [{ visible: 'conversation' }]
		}
	]
});
