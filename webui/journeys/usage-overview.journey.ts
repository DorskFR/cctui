import { defineJourney } from '@dorsk/journey';

const HOME = '/';

export default defineJourney({
	id: 'usage-overview',
	title: { en: 'See what the fleet is costing', fr: 'Voir ce que coûte la flotte' },
	description: { en: 'The overview answers how many agents are running and where the tokens went.', fr: 'La vue d’ensemble répond à deux questions : combien d’agents tournent, et où sont passés les jetons.' },
	route: HOME,
	variants: { viewport: ['desktop', 'mobile'], theme: ['dark'] },
	level: 'checked',
	steps: [
		{
			id: 'tiles',
			route: HOME,
			target: 'tiles',
			say: {
				title: { en: 'The fleet in four numbers', fr: 'La flotte en quatre chiffres' },
				body: { en: 'Sessions running now, sessions waiting on you, machines online, and the all-time total. They read zero until your first run.', fr: 'Les sessions en cours, celles qui vous attendent, les machines en ligne et le total depuis toujours. Tout est à zéro jusqu’à votre premier run.' }
			},
			expect: [
				{ visible: 'tiles' },
				{ visible: 'tile[needs_input]' },
				{ visible: 'tile[machines]' }
			],
			capture: 'tiles'
		},
		{
			id: 'attention',
			route: HOME,
			target: 'tile[needs_input]',
			say: {
				title: { en: 'The one number to act on', fr: 'Le chiffre sur lequel agir' },
				body: { en: 'This counts agents that have stopped and are waiting for an answer from you. Work is not progressing while it is above zero, so read this tile first.', fr: 'Ce compteur indique les agents arrêtés qui attendent une réponse de votre part. Tant qu’il n’est pas à zéro, le travail n’avance pas : lisez cette tuile en premier.' }
			},
			expect: [{ visible: 'tile[needs_input]' }]
		},
		{
			id: 'capacity',
			route: HOME,
			target: 'tile[machines]',
			say: {
				title: { en: 'How much capacity you have', fr: 'La capacité dont vous disposez' },
				body: { en: 'Online machines over enrolled machines. A machine that has stopped reporting still counts as enrolled, so a gap between the two numbers is where a new session would fail to land.', fr: 'Machines en ligne sur machines enrôlées. Une machine qui ne répond plus reste enrôlée : l’écart entre les deux chiffres indique où une nouvelle session ne pourrait pas démarrer.' }
			},
			expect: [{ visible: 'tile[machines]' }]
		},
		{
			id: 'periods',
			route: HOME,
			target: 'session-periods',
			say: {
				title: { en: 'Is today busier than usual?', fr: 'La journée est-elle plus chargée que d’habitude ?' },
				body: { en: 'The same count over today, yesterday, this week and this month. One day on its own means little; next to the others it tells you whether the fleet is speeding up.', fr: 'Le même décompte pour aujourd’hui, hier, cette semaine et ce mois. Une journée seule ne dit pas grand-chose ; comparée aux autres, elle indique si la flotte accélère.' }
			},
			expect: [{ visible: 'session-periods' }, { visible: 'session-periods/period[today]' }],
			capture: 'periods'
		},
		{
			id: 'windows',
			route: HOME,
			target: 'windows',
			say: {
				title: { en: 'Tokens by time window', fr: 'Jetons par fenêtre de temps' },
				body: { en: 'The same usage read over the last hour, day, week and month, split into input, output and cached tokens. Cached tokens are re-read context and cost a fraction of fresh input.', fr: 'La même consommation sur la dernière heure, le dernier jour, la dernière semaine et le dernier mois, répartie en jetons d’entrée, de sortie et mis en cache. Les jetons mis en cache sont du contexte relu et coûtent une fraction de l’entrée neuve.' }
			},
			expect: [{ visible: 'windows' }],
			capture: 'windows'
		},
		{
			id: 'range',
			route: HOME,
			target: 'range',
			say: {
				title: { en: 'Choose the period you are reading', fr: 'Choisir la période que vous lisez' },
				body: { en: 'This selector governs the charts below, not the tiles above. Widen it to judge a trend, narrow it to explain a single expensive day.', fr: 'Ce sélecteur gouverne les graphiques ci-dessous, pas les tuiles ci-dessus. Élargissez-le pour juger une tendance, réduisez-le pour expliquer une journée coûteuse.' }
			},
			expect: [{ visible: 'range' }]
		},
		{
			id: 'analytics',
			route: HOME,
			target: 'analytics',
			say: {
				title: { en: 'Where the tokens went', fr: 'Où sont passés les jetons' },
				body: { en: 'Daily volume and a per-model breakdown, so an expensive habit shows up before the bill does. A model you did not mean to use is usually visible here first.', fr: 'Le volume quotidien et une répartition par modèle, pour qu’une habitude coûteuse se voie avant la facture. Un modèle utilisé par erreur se repère généralement ici en premier.' }
			},
			expect: [{ visible: 'analytics' }],
			capture: 'analytics'
		}
	]
});
