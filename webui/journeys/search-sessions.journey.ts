import { defineJourney } from '@dorsk/journey';

// Tsumikit's FilterSearchBar forwards no attributes to its input, so this
// target is an accessible name rather than a `data-journey` path.
const BOX = { label: 'Search sessions', within: 'search' } as const;

export default defineJourney({
	id: 'search-sessions',
	title: 'Find a session again',
	description: 'Search runs over every transcript, and narrows by machine, label or status.',
	route: '/sessions',
	variants: { viewport: ['desktop', 'mobile'], theme: ['dark'] },
	level: 'checked',
	steps: [
		{
			id: 'start',
			route: '/sessions',
			target: 'section[blocked]',
			say: {
				title: 'Start from the whole list',
				body: 'Every session you have run is searchable, live ones and finished ones alike.'
			},
			expect: [{ count: ['session', { min: 4 }] }],
			capture: 'before'
		},
		{
			id: 'free-text',
			target: BOX,
			do: { kind: 'fill', value: 'pagination' },
			say: {
				title: 'Search the transcripts, not just the titles',
				body: 'A plain word is matched against what the agents actually said and did, so you can find a run by what it touched.'
			},
			expect: [{ count: ['session', { equals: 1 }] }],
			capture: 'text'
		},
		{
			id: 'facet',
			target: BOX,
			do: { kind: 'fill', value: 'label:backend' },
			say: {
				title: 'Narrow by label, machine or status',
				body: 'Typed filters like label:, machine: and status: combine with the free text to cut a large fleet down fast.'
			},
			expect: [{ count: ['session', { min: 2 }] }],
			capture: 'facet'
		}
	]
});
