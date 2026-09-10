import { defineJourney } from '@dorsk/journey';

export default defineJourney({
	id: 'usage-overview',
	title: { en: 'See what the fleet is costing', fr: 'Voir ce que coûte la flotte' },
	description: { en: 'The overview answers how many agents are running and where the tokens went.', fr: 'La vue d’ensemble répond à deux questions : combien d’agents tournent, et où sont passés les jetons.' },
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
				body: 'Four numbers: sessions running now, sessions waiting on you, machines online, and the all-time total. They read zero until your first run.'
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
				title: { en: 'Tokens by time window', fr: 'Jetons par fenêtre de temps' },
				body: { en: 'The same usage read over the last hour, day, week and month, split into input, output and cached tokens.', fr: 'La même consommation lue sur la dernière heure, le dernier jour, la dernière semaine et le dernier mois, répartie en jetons d’entrée, de sortie et mis en cache.' }
			},
			expect: [{ visible: 'windows' }],
			capture: 'windows'
		},
		{
			id: 'analytics',
			target: 'analytics',
			say: {
				title: { en: 'Where the tokens went', fr: 'Où sont passés les jetons' },
				body: { en: 'Daily volume and a per-model breakdown, so an expensive habit shows up before the bill does.', fr: 'Le volume quotidien et une répartition par modèle, pour qu’une habitude coûteuse se voie avant la facture.' }
			},
			expect: [{ visible: 'analytics' }],
			capture: 'analytics'
		}
	]
});
