import { defineJourney } from '@dorsk/journey';

// Cards and message lines repeat, so a bare path matches every one of them and
// throws. `nth` indexes the visible matches and exists only on the locator form.
const first = (path: string[]) =>
	({ css: path.map((n) => `[data-journey="${n}"]`).join(' '), nth: 0 }) as const;
const FIRST_SESSION_TITLE = first(['session', 'title']);
const FIRST_LINE = first(['conversation', 'line']);
const FIRST_LINE_ACTIONS = first(['conversation', 'line', 'line-actions']);

export default defineJourney({
	id: 'follow-session',
	title: { en: 'Follow a session while it works', fr: 'Suivre une session pendant son travail' },
	description: { en: 'Open a running agent, read what it did, and reply without leaving the list.', fr: 'Ouvrez un agent en cours, lisez ce qu’il a fait et répondez sans quitter la liste.' },
	route: '/sessions',
	fixture: 'instance',
	variants: { viewport: ['desktop', 'mobile'], theme: ['dark'] },
	level: 'checked',
	steps: [
		{
			id: 'open',
			route: '/sessions',
			// Not a session id snapshotted at start: it is gone the moment that
			// session ends, is archived, or scrolls out of view.
			target: FIRST_SESSION_TITLE,
			do: { kind: 'click' },
			guide: 'next',
			say: {
				title: { en: 'Open a session', fr: 'Ouvrir une session' },
				body: {
					en: 'Open one by its name. The conversation slides in beside the list on a desktop and over it on a phone, so you never lose your place in the fleet.',
					fr: 'Ouvrez-en une par son nom. La conversation s’ouvre à côté de la liste sur un ordinateur, par-dessus sur un téléphone : vous ne perdez jamais votre place dans la flotte.'
				}
			},
			expect: [{ visible: 'conversation' }, { visible: 'composer' }],
			capture: 'drawer'
		},
		{
			id: 'header',
			target: 'conversation/header',
			say: {
				title: { en: 'Who is running this, and where', fr: 'Qui exécute ceci, et où' },
				body: {
					en: 'The top row answers the questions you ask first: is it alive, which machine is it on, which account is paying for it, and what is it called.',
					fr: 'La première ligne répond aux questions qu’on se pose d’abord : est-elle vivante, sur quelle machine tourne-t-elle, quel compte la paie, et comment s’appelle-t-elle.'
				}
			},
			expect: [{ visible: 'conversation/header' }],
			capture: 'header'
		},
		{
			id: 'meta',
			target: 'conversation/head-meta',
			say: {
				title: { en: 'What it is working on, and what it has spent', fr: 'Sur quoi elle travaille, et ce qu’elle a dépensé' },
				body: {
					en: 'The second row is the run’s cost and context: working directory, git branch, model, and the token usage so far. Watch it when a session starts feeling slow or expensive.',
					fr: 'La seconde ligne donne le coût et le contexte : répertoire de travail, branche git, modèle et jetons consommés. Surveillez-la quand une session devient lente ou coûteuse.'
				}
			},
			expect: [{ visible: 'conversation/head-meta' }]
		},
		{
			id: 'details',
			target: 'conversation/head-details',
			say: {
				title: { en: 'The rest of the detail folds away', fr: 'Le reste du détail se replie' },
				body: {
					en: 'Everything that would crowd the header — the full path, the session id, its parent if it was forked — lives behind this toggle, so the two rows above stay readable.',
					fr: 'Tout ce qui encombrerait l’en-tête — le chemin complet, l’identifiant de session, son parent en cas de bifurcation — se trouve derrière ce bouton, pour que les deux lignes ci-dessus restent lisibles.'
				}
			},
			expect: [{ visible: 'conversation/head-details' }],
			capture: 'details'
		},
		{
			id: 'activity',
			target: 'activity',
			say: {
				title: { en: 'What it is doing right now', fr: 'Ce qu’elle fait en ce moment' },
				body: {
					en: 'While a turn is live this strip names the step in progress, the tool it is running, how long the turn has taken and how far through its task list it is. Between turns it simply reads idle — which is how you tell a thinking agent from a finished one.',
					fr: 'Pendant qu’un tour est en cours, cette bande nomme l’étape en progrès, l’outil exécuté, la durée du tour et l’avancement de sa liste de tâches. Entre deux tours, elle indique simplement « au repos » — c’est ainsi qu’on distingue un agent qui réfléchit d’un agent qui a fini.'
				}
			},
			expect: [{ visible: 'activity' }],
			capture: 'activity'
		},
		{
			id: 'actions',
			target: 'conversation/actions',
			say: {
				title: { en: 'Branch instead of starting over', fr: 'Bifurquer plutôt que tout recommencer' },
				body: {
					en: 'The ⋯ menu holds the less-used actions: fork, which copies the history up to a message and continues from there — how you try a second approach without losing the first — plus copy a link, export, and the read-only live terminal.',
					fr: 'Le menu ⋯ regroupe les actions moins courantes : bifurquer, qui copie l’historique jusqu’à un message et repart de là — pour tenter une seconde approche sans perdre la première —, copier un lien, exporter et le terminal en direct en lecture seule.'
				}
			},
			expect: [{ visible: 'conversation/actions' }]
		},
		{
			id: 'kinds',
			target: FIRST_LINE,
			say: {
				title: { en: 'Everything it did is on the record', fr: 'Tout ce qu’elle a fait est consigné' },
				body: {
					en: 'Each message is badged with its kind: your prompts, the agent’s replies, its reasoning, every tool call and the result that came back. Nothing is summarised away.',
					fr: 'Chaque message porte son type : vos prompts, les réponses de l’agent, son raisonnement, chaque appel d’outil et le résultat renvoyé. Rien n’est résumé ni masqué.'
				}
			},
			expect: [{ visible: FIRST_LINE }],
			capture: 'timeline'
		},
		{
			id: 'line-actions',
			target: FIRST_LINE_ACTIONS,
			say: {
				title: { en: 'Lift one message out', fr: 'Extraire un message' },
				body: {
					en: 'Any single message can be pinned to find again, copied as Markdown for a ticket, or saved as an image to paste into a review.',
					fr: 'N’importe quel message peut être épinglé pour le retrouver, copié en Markdown pour un ticket, ou enregistré en image à coller dans une revue.'
				}
			},
			expect: [{ visible: FIRST_LINE_ACTIONS }],
			capture: 'line'
		},
		{
			id: 'filters',
			target: 'filters',
			say: {
				title: { en: 'Hide the noise', fr: 'Masquer le bruit' },
				body: {
					en: 'These pills hide whole kinds of message. Turning the assistant off leaves only the tool calls — the quickest way to see what an agent actually touched.',
					fr: 'Ces pastilles masquent des types entiers de messages. Désactiver l’assistant ne laisse que les appels d’outils — le moyen le plus rapide de voir ce que l’agent a réellement touché.'
				}
			},
			expect: [{ visible: 'filters/quick[assistant]' }]
		},
		{
			id: 'filter-menu',
			target: 'filters/filter-menu',
			say: {
				title: { en: 'Or pick the categories yourself', fr: 'Ou choisir les catégories vous-même' },
				body: {
					en: 'The pills are shortcuts over a finer list. Open it when you want one tool kind and nothing else — reading only the file writes, for instance.',
					fr: 'Les pastilles sont des raccourcis sur une liste plus fine. Ouvrez-la pour ne garder qu’un seul type d’outil — les écritures de fichiers, par exemple.'
				}
			},
			expect: [{ visible: 'filters/filter-menu' }]
		},
		{
			id: 'tools-only',
			target: 'filters/quick[assistant]',
			do: { kind: 'click' },
			guide: 'next',
			say: {
				title: { en: 'Try it: leave only what it touched', fr: 'Essayez : ne gardez que ce qu’elle a touché' },
				body: {
					en: 'Turn the assistant pill off. The prose disappears and the tool calls remain — the fastest way to audit what an agent actually did to your files.',
					fr: 'Désactivez la pastille « assistant ». La prose disparaît, les appels d’outils restent — le moyen le plus rapide d’auditer ce que l’agent a vraiment fait à vos fichiers.'
				}
			},
			expect: [{ hidden: 'conversation/line[assistant]' }],
			capture: 'tools'
		},
		{
			id: 'tools-restore',
			target: 'filters/quick[assistant]',
			do: { kind: 'click' },
			guide: 'next',
			say: {
				title: { en: 'And put it back', fr: 'Et remettez-la' },
				body: {
					en: 'Click it again. Filters only ever change what this pane shows you — nothing was removed from the transcript, and the setting does not follow you to the next session.',
					fr: 'Recliquez. Les filtres ne changent que ce que ce panneau affiche — rien n’a été retiré de la transcription, et le réglage ne vous suit pas dans la session suivante.'
				}
			},
			expect: [{ visible: 'conversation/line[assistant]' }],
			capture: 'restored'
		},
		{
			id: 'reply',
			target: 'composer/message',
			say: {
				title: { en: 'Steer it from here', fr: 'La piloter d’ici' },
				body: {
					en: 'Anything you type goes to the running agent, so you can redirect it mid-task instead of stopping it and starting again.',
					fr: 'Ce que vous tapez part vers l’agent en cours : vous pouvez le réorienter en pleine tâche au lieu de l’arrêter et de recommencer.'
				}
			},
			expect: [{ visible: 'composer/message' }],
			capture: 'reply'
		}
	]
});
