import { defineJourney } from '@dorsk/journey';

const LOOSE = 'acme-research';

export default defineJourney({
	id: 'accounts-pools',
	title: 'Connect a provider account',
	description:
		'Accounts are the credentials work runs on. Add one so a session has something to run with.',
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
				body: 'Accounts are the provider credentials your agents run on. This board is empty until you add one.'
			},
			expect: [{ visible: { role: 'heading', name: 'Accounts' } }, { visible: 'accounts' }],
			capture: 'board'
		},
		{
			id: 'add',
			target: 'new-account',
			do: { kind: 'click' },
			say: {
				title: 'Add your first account',
				body: 'Add your first account. Once it is saved, this guide is complete; pools are for when you have more than one.'
			}
		},
		{
			id: 'pool',
			qaOnly: true,
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
			qaOnly: true,
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
			qaOnly: true,
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
