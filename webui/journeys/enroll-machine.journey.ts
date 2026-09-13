import { defineJourney } from '@dorsk/journey';

const ACCESS = '/access';

export default defineJourney({
	id: 'enroll-machine',
	title: { en: 'Bring a machine into the fleet', fr: 'Enrôler une machine dans la flotte' },
	description: { en: 'Access holds the people, their keys and the machines that run their agents.', fr: 'Accès regroupe les personnes, leurs clés et les machines qui exécutent leurs agents.' },
	route: ACCESS,
	fixture: 'instance',
	variants: { viewport: ['desktop', 'mobile'], theme: ['dark'] },
	level: 'checked',
	steps: [
		{
			id: 'access',
			route: ACCESS,
			target: 'enroll',
			say: {
				title: { en: 'Start here: enroll a machine', fr: 'Commencez ici : enrôler une machine' },
				body: { en: 'Access lists everyone and everything that can act on this instance. Until a machine has enrolled, this card is the only thing here that matters.', fr: 'Accès liste tout ce qui peut agir sur cette instance. Tant qu’aucune machine n’est enrôlée, cette carte est la seule chose qui compte ici.' }
			},
			expect: [{ visible: 'enroll' }],
			capture: 'access'
		},
		{
			id: 'command',
			route: ACCESS,
			target: 'enroll-cmd',
			say: {
				title: { en: 'What this command does', fr: 'Ce que fait cette commande' },
				body: { en: 'It points the daemon at this server and registers the machine under your user. Swap the token placeholder for one of your own keys — that token is what ties the machine to you.', fr: 'Elle pointe le démon vers ce serveur et enregistre la machine sous votre utilisateur. Remplacez le jeton d’exemple par une de vos clés : c’est lui qui rattache la machine à vous.' }
			},
			expect: [{ visible: 'enroll-cmd' }],
			capture: 'command'
		},
		{
			id: 'copy',
			route: ACCESS,
			target: 'enroll-copy',
			say: {
				title: { en: 'Run it on the machine itself', fr: 'Exécutez-la sur la machine elle-même' },
				body: { en: 'Copy it and run it on the computer that will host your agents — not here. Install it as a service afterwards so the machine rejoins the fleet on its own after a reboot.', fr: 'Copiez-la et exécutez-la sur l’ordinateur qui hébergera vos agents, pas ici. Installez-la ensuite comme service pour que la machine rejoigne seule la flotte après un redémarrage.' }
			},
			expect: [{ visible: 'enroll-copy' }]
		},
		{
			id: 'enroll',
			route: ACCESS,
			target: 'enroll',
			// A probe wait is bounded only by the step timeout, and enrolling a
			// machine takes far longer than the 10 s default.
			timeout: 600000,
			optional: true,
			say: {
				title: { en: 'Waiting for the machine to report in', fr: 'En attente du rapport de la machine' },
				body: { en: 'The guide moves on by itself the moment the machine checks in. If you would rather finish setting it up later, leave this step and come back — nothing is lost.', fr: 'Le guide avance dès que la machine se signale. Si vous préférez terminer plus tard, quittez cette étape et revenez : rien n’est perdu.' }
			},
			expect: [{ visible: 'enroll' }, { probe: 'machines.online' }],
			capture: 'enroll'
		},
		{
			id: 'user',
			route: ACCESS,
			target: 'user[{fixture.me}]',
			do: { kind: 'click' },
			say: {
				title: { en: 'Open your own user', fr: 'Ouvrez votre utilisateur' },
				body: { en: 'Everything attached to an identity is here: the keys it signs in with, the machines it enrolled, its tokens and its AI accounts.', fr: 'Tout ce qui est rattaché à une identité se trouve ici : ses clés de connexion, les machines qu’elle a enrôlées, ses jetons et ses comptes IA.' }
			},
			expect: [{ visible: 'tab' }],
			capture: 'user'
		},
		{
			id: 'tabs',
			route: ACCESS,
			target: 'tab',
			say: {
				title: { en: 'One tab per kind of credential', fr: 'Un onglet par type d’identifiant' },
				body: { en: 'Keys sign a person in, tokens let a machine enroll, and accounts are what the agents spend. Revoking any of them takes effect immediately, which is how you retire a lost laptop.', fr: 'Les clés connectent une personne, les jetons permettent à une machine de s’enrôler, et les comptes sont ce que dépensent les agents. Révoquer l’un d’eux prend effet immédiatement : c’est ainsi qu’on retire un portable perdu.' }
			},
			expect: [{ visible: 'tab' }]
		},
		{
			id: 'machines',
			// The tab name embeds the fixture's machine count and tsumikit's Tabs
			// forwards no anchor to its triggers, so the step stays in the book.
			qaOnly: true,
			target: { role: 'tab', name: 'Machines 2' },
			do: { kind: 'click' },
			say: {
				title: { en: 'The machines that answered', fr: 'Les machines qui ont répondu' },
				body: { en: 'The machine you just enrolled is listed here with its heartbeat. Online means it can host a session right now.', fr: 'La machine que vous venez d’enrôler figure ici avec son signal de vie. « En ligne » signifie qu’elle peut héberger une session immédiatement.' }
			},
			expect: [{ visible: 'tab[machines]' }],
			capture: 'machines'
		}
	]
});
