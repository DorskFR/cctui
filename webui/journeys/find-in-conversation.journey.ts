import { defineJourney } from '@dorsk/journey';

// Cards repeat, so a bare path matches every one of them and throws.
const FIRST_SESSION_TITLE = {
	css: '[data-journey="session"] [data-journey="title"]',
	nth: 0
} as const;
// Tsumikit's FilterSearchBar forwards no attributes to its input, so the bar is
// addressed by accessible name within the cctui wrapper.
const FIND_BOX = { label: 'Search in conversation', within: 'conversation-search' } as const;
const SESSIONS = '/sessions';

export default defineJourney({
	id: 'find-in-conversation',
	title: { en: 'Find something inside one conversation', fr: 'Retrouver quelque chose dans une conversation' },
	description: { en: 'The same search box, scoped to one transcript, counting matches the loaded page has not reached yet.', fr: 'La même boîte de recherche, limitée à une seule transcription, qui compte aussi les résultats hors de la page chargée.' },
	route: SESSIONS,
	variants: { viewport: ['desktop', 'mobile'], theme: ['dark'] },
	level: 'checked',
	steps: [
		{
			id: 'open-conversation',
			qaOnly: true,
			route: SESSIONS,
			target: FIRST_SESSION_TITLE,
			do: { kind: 'click' },
			say: {
				title: { en: 'Open a conversation', fr: 'Ouvrir une conversation' },
				body: { en: 'Searching inside one starts from having it open.', fr: 'Chercher dans une conversation commence par l’ouvrir.' }
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
				body: { en: 'The bar opens under the toolbar, focused and ready. ⌘F / Ctrl+F does the same.', fr: 'La barre s’ouvre sous la barre d’outils, prête à recevoir le curseur. ⌘F / Ctrl+F fait de même.' }
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
				body: { en: 'Matches older than the loaded page are counted too, so the total covers the whole transcript, not the part on screen.', fr: 'Les résultats plus anciens que la page chargée sont comptés aussi : le total couvre toute la transcription, pas seulement l’écran.' }
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
				body: { en: 'The bar closes and the highlight goes with it; the conversation stays where you were reading.', fr: 'La barre se ferme avec la surbrillance ; la conversation reste là où vous lisiez.' }
			},
			expect: [{ visible: 'conversation' }]
		}
	]
});
