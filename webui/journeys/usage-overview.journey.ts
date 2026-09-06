import { defineJourney } from '@dorsk/journey';

export default defineJourney({
	id: 'usage-overview',
	title: 'See what the fleet is costing',
	description: 'The overview answers how many agents are running and where the tokens went.',
	route: '/',
	variants: { viewport: ['desktop', 'mobile'], theme: ['dark'] },
	level: 'checked',
	steps: [
		{
			id: 'tiles',
			route: '/',
			target: 'tiles',
			say: {
				title: 'The fleet in four numbers',
				body: 'Sessions running now, sessions waiting on you, machines online, and the all-time total.'
			},
			expect: [
				{ visible: 'tiles' },
				{ visible: 'tile[needs_input]' },
				{ visible: 'tile[machines]' }
			],
			capture: 'tiles'
		},
		{
			id: 'windows',
			target: 'windows',
			say: {
				title: 'Tokens by time window',
				body: 'The same usage read over the last hour, day, week and month, split into input, output and cached tokens.'
			},
			expect: [{ visible: 'windows' }],
			capture: 'windows'
		},
		{
			id: 'analytics',
			target: 'analytics',
			say: {
				title: 'Where the tokens went',
				body: 'Daily volume and a per-model breakdown, so an expensive habit shows up before the bill does.'
			},
			expect: [{ visible: 'analytics' }],
			capture: 'analytics'
		}
	]
});
