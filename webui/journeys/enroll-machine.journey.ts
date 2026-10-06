import { defineJourney } from '@dorsk/journey';

const ACCESS = '/settings/users';

export default defineJourney({
	id: 'enroll-machine',
	title: { en: 'Bring a machine into the fleet', fr: 'Enrôler une machine dans la flotte' },
	description: { en: 'Users & keys holds the people, their keys and the machines that run their agents.', fr: 'Utilisateurs et clés regroupe les personnes, leurs clés et les machines qui exécutent leurs agents.' },
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
				title: { en: 'A machine is where an agent actually runs', fr: 'Une machine est là où un agent s’exécute réellement' },
				body: { en: 'cctui itself runs nothing. A small daemon on your own computer does, and enrolling is how that computer tells this server it exists. Until one has enrolled, this card is the only thing here that matters.', fr: 'cctui n’exécute rien lui-même. Un petit démon sur votre ordinateur s’en charge, et l’enrôlement est la façon dont cet ordinateur signale son existence à ce serveur. Tant qu’aucune machine n’est enrôlée, cette carte est la seule chose qui compte ici.' }
			},
			expect: [{ visible: 'enroll' }],
			capture: 'access'
		},
		{
			id: 'command',
			target: 'enroll-cmd',
			say: {
				title: { en: 'What this command does', fr: 'Ce que fait cette commande' },
				body: { en: 'It points the daemon at this server and registers the machine under your user. The token in it is what ties the machine to you — swap the placeholder for one of your own keys.', fr: 'Elle pointe le démon vers ce serveur et enregistre la machine sous votre utilisateur. Le jeton qu’elle contient rattache la machine à vous — remplacez l’exemple par une de vos clés.' }
			},
			expect: [{ visible: 'enroll-cmd' }],
			capture: 'command'
		},
		{
			id: 'copy',
			target: 'enroll-copy',
			do: { kind: 'click' },
			guide: 'next',
			say: {
				title: { en: 'Copy it, then run it over there', fr: 'Copiez-la, puis exécutez-la là-bas' },
				body: { en: 'Take the copy now. It has to run on the computer that will host your agents, not in this browser. Install it as a service afterwards and the machine rejoins the fleet by itself after a reboot.', fr: 'Copiez-la maintenant. Elle doit s’exécuter sur l’ordinateur qui hébergera vos agents, pas dans ce navigateur. Installez-la ensuite comme service et la machine rejoindra seule la flotte après un redémarrage.' }
			},
			capture: 'copy'
		},
		{
			id: 'online',
			target: 'enroll',
			say: {
				title: { en: 'How you will know it worked', fr: 'Comment savoir que ça a marché' },
				body: { en: 'A machine that has checked in appears under your user as Online, with its last heartbeat. Online is the whole test: it means the machine can host a session right now. Nothing here waits on it — come back whenever the daemon is up.', fr: 'Une machine qui s’est signalée apparaît sous votre utilisateur comme « En ligne », avec son dernier signal de vie. « En ligne » est le seul critère : la machine peut héberger une session immédiatement. Rien ici ne l’attend — revenez quand le démon tourne.' }
			},
			capture: 'online'
		},
		{
			id: 'user',
			target: 'user[{fixture.me}]',
			do: { kind: 'click' },
			guide: 'next',
			say: {
				title: { en: 'Open your own user', fr: 'Ouvrez votre utilisateur' },
				body: { en: 'Everything attached to an identity lives here: the keys it signs in with, the machines it enrolled, its tokens, and the AI accounts its agents spend.', fr: 'Tout ce qui est rattaché à une identité se trouve ici : ses clés de connexion, les machines qu’elle a enrôlées, ses jetons, et les comptes IA que ses agents dépensent.' }
			},
			expect: [{ visible: 'tab' }],
			capture: 'user'
		},
		{
			id: 'tabs',
			target: 'tab',
			say: {
				title: { en: 'One panel per kind of credential', fr: 'Un panneau par type d’identifiant' },
				body: { en: 'Keys sign a person in, tokens let a machine enroll, and accounts are what the agents spend. Revoking any of them takes effect immediately — that is how you retire a lost laptop.', fr: 'Les clés connectent une personne, les jetons permettent à une machine de s’enrôler, et les comptes sont ce que dépensent les agents. Révoquer l’un d’eux prend effet immédiatement — c’est ainsi qu’on retire un portable perdu.' }
			},
			expect: [{ visible: 'tab' }],
			capture: 'tabs'
		}
	]
});
