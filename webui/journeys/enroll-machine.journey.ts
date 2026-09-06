import { defineJourney } from '@dorsk/journey';

export default defineJourney({
	id: 'enroll-machine',
	title: 'Bring a machine into the fleet',
	description: 'Access holds the people, their keys and the machines that run their agents.',
	route: '/access',
	variants: { viewport: ['desktop', 'mobile'], theme: ['dark'] },
	level: 'checked',
	steps: [
		{
			id: 'access',
			route: '/access',
			target: 'user[admin]',
			say: {
				title: 'People, keys and machines',
				body: 'Everything that can act on this instance is listed here, one user at a time.'
			},
			expect: [{ visible: 'user[admin]' }, { visible: 'enroll' }],
			capture: 'access'
		},
		{
			id: 'enroll',
			target: 'enroll',
			say: {
				title: 'One command per machine',
				body: 'A machine joins by running the daemon with a user token; from then on it can host sessions.'
			},
			expect: [{ visible: 'enroll' }],
			capture: 'enroll'
		},
		{
			id: 'user',
			target: 'user[admin]',
			do: { kind: 'click' },
			say: {
				title: 'Open a user',
				body: 'Each user carries their own API keys, machines, tokens and AI accounts.'
			},
			expect: [{ visible: 'tab[keys]' }],
			capture: 'user'
		},
		{
			id: 'machines',
			target: { role: 'tab', name: 'Machines 2' },
			do: { kind: 'click' },
			say: {
				title: 'The machines that answered',
				body: 'Enrolled machines report in with a heartbeat, so you know which are online.'
			},
			expect: [{ visible: 'tab[machines]' }],
			capture: 'machines'
		}
	]
});
