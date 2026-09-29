import { defineJourney } from '@dorsk/journey';

// Tsumikit's FilterSearchBar forwards no attributes to its input, so the field is
// addressed by accessible name within the cctui wrapper.
const BOX = { label: 'Search sessions', within: 'search' } as const;
const SESSIONS = '/sessions';

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
			id: 'whole',
			target: 'session-list',
			say: {
				title: { en: 'Everything you have run is in here', fr: 'Tout ce que vous avez lancé est ici' },
				body: { en: 'Live runs and finished ones alike. Count the rows now — the next two steps will cut this list down in front of you.', fr: 'Les runs en cours comme ceux terminés. Comptez les lignes maintenant : les deux étapes suivantes vont réduire cette liste sous vos yeux.' }
			},
			expect: [{ count: ['session', { min: 4 }] }],
			capture: 'before'
		},
		{
			id: 'free-text',
			target: BOX,
			do: { kind: 'fill', value: { $param: 'var.query' } },
			say: {
				title: { en: 'Type a word the run would have used', fr: 'Tapez un mot que le run aurait employé' },
				body: { en: 'Try “pagination”. A plain word is matched against the transcripts themselves, so you find a session by what it touched rather than by what you called it.', fr: 'Essayez « pagination ». Un simple mot est comparé aux transcriptions elles-mêmes : vous retrouvez une session par ce qu’elle a touché plutôt que par son nom.' }
			},
			capture: 'text'
		},
		{
			id: 'facet',
			target: BOX,
			do: { kind: 'fill', value: { $param: 'var.facet' } },
			say: {
				title: { en: 'Turn a word into a condition', fr: 'Transformer un mot en condition' },
				body: { en: 'Replace it with “label:backend”. Prefixes like label:, machine: and status: filter instead of searching, and the box offers the values it already knows.', fr: 'Remplacez-le par « label:backend ». Les préfixes comme label:, machine: et status: filtrent au lieu de chercher, et la boîte propose les valeurs qu’elle connaît déjà.' }
			},
			capture: 'facet'
		},
		{
			id: 'combine',
			target: 'session-list',
			say: {
				title: { en: 'Stack them to answer a real question', fr: 'Les combiner pour répondre à une vraie question' },
				body: { en: 'Conditions and free text apply together: status:failed with a word finds the run that broke on the thing you half-remember. That is the whole query language.', fr: 'Conditions et texte libre s’appliquent ensemble : status:failed plus un mot retrouve le run qui a cassé sur ce dont vous vous souvenez à moitié. C’est tout le langage de requête.' }
			},
			capture: 'combined'
		},
		{
			id: 'clear',
			target: BOX,
			do: { kind: 'fill', value: { $param: 'var.blank' } },
			say: {
				title: { en: 'Empty the box to get the fleet back', fr: 'Videz la boîte pour retrouver la flotte' },
				body: { en: 'Clear it and every session returns. Nothing was hidden or archived — search only ever changes what this list shows you.', fr: 'Videz-la et toutes les sessions reviennent. Rien n’a été masqué ni archivé : la recherche ne change que ce que cette liste vous montre.' }
			},
			capture: 'cleared'
		}
	]
});
