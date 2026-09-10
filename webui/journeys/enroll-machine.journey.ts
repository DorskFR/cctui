import { defineJourney } from '@dorsk/journey';

export default defineJourney({
	id: 'enroll-machine',
	title: 'Bring a machine into the fleet',
	description: 'Access holds the people, their keys and the machines that run their agents.',
	route: '/access',
	fixture: 'instance',
	variants: { viewport: ['desktop', 'mobile'], theme: ['dark'] },
	level: 'checked',
	steps: [
		{
			id: 'access',
			route: '/access',
			target: 'enroll',
			say: {
				title: 'Start here: enroll a machine',
				body: 'Access lists everyone and everything that can act on this instance. Until a machine has enrolled, this card is the only thing here that matters.'
			},
			expect: [{ visible: 'enroll' }],
			capture: 'access'
		},
		{
			id: 'enroll',
			target: 'enroll',
			// A probe wait is bounded only by the step timeout, and enrolling a
			// machine takes far longer than the 10 s default.
			timeout: 600000,
			say: {
				title: 'Run this on the machine',
				body: 'Copy this command and run it on the computer that will host your agents. Replace the token with one from your user. The guide moves on by itself when the machine reports in.'
			},
			expect: [{ visible: 'enroll' }, { probe: 'machines.online' }],
			capture: 'enroll'
		},
		{
			id: 'user',
			target: 'user[{fixture.me}]',
			do: { kind: 'click' },
			say: {
				title: 'Open a user',
				body: 'Open your own user. Keys, machines, tokens and AI accounts each have a tab.'
			},
			expect: [{ visible: 'tab[keys]' }],
			capture: 'user'
		},
		{
			id: 'machines',
			// The tab name embeds the fixture's machine count and tsumikit's Tabs
			// forwards no anchor to its triggers, so the step stays in the book.
			qaOnly: true,
			target: { role: 'tab', name: 'Machines 2' },
			do: { kind: 'click' },
			say: {
				title: 'The machines that answered',
				body: 'The machine you just enrolled is listed here with its heartbeat. Online means it can host a session right now.'
			},
			expect: [{ visible: 'tab[machines]' }],
			capture: 'machines'
		}
	]
});
