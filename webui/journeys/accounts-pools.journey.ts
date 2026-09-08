import { defineJourney } from '@dorsk/journey';

const LOOSE = 'acme-research';

export default defineJourney({
	id: 'accounts-pools',
	title: 'Group accounts into a pool',
	description:
		'Accounts are the credentials work runs on; a pool makes several of them interchangeable so a launch can pick whichever has headroom.',
	route: '/accounts',
	variants: { viewport: ['desktop', 'mobile'], theme: ['dark'] },
	level: 'checked',
	steps: [
		{
			id: 'board',
			route: '/accounts',
			target: 'accounts',
			say: {
				title: 'Every account you can run on',
				body: 'One card per account, each showing the providers it carries. Pools sit alongside them as drop zones.'
			},
			expect: [
				{ visible: { role: 'heading', name: 'Accounts' } },
				{ visible: 'accounts' },
				{ visible: `account[${LOOSE}]` }
			],
			capture: 'board'
		},
		{
			id: 'pool',
			target: 'pool[production]',
			say: {
				title: 'A pool is a set of interchangeable accounts',
				body: 'Launches aimed at the pool elect a member by headroom, so a spent weekly budget on one account does not stop the work.'
			},
			expect: [{ visible: 'pool[production]' }],
			capture: 'pool'
		},
		{
			id: 'handle',
			target: `account[${LOOSE}]/drag-handle`,
			say: {
				title: 'Drag an account into a pool',
				body: 'The grip on a card lifts it; dropping it on a pool adds it to the membership.'
			},
			expect: [{ visible: `account[${LOOSE}]/drag-handle` }],
			capture: 'handle'
		},
		{
			id: 'menu',
			target: `account[${LOOSE}]/account-menu`,
			do: { kind: 'click' },
			say: {
				title: 'Or add it from the card menu',
				body: 'The same membership change without a mouse drag, which is also how it is done on a phone.'
			},
			capture: 'menu'
		}
	]
});
