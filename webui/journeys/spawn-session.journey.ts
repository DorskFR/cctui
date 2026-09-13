import { defineJourney, param } from '@dorsk/journey';

const SESSIONS = '/sessions';

export default defineJourney({
	id: 'spawn-session',
	title: { en: 'Start a new agent', fr: 'Lancer un nouvel agent' },
	description: { en: 'Describe the work, pick where it runs, and keep it as a draft until you are ready.', fr: 'Décrivez le travail, choisissez où il s’exécute, et gardez-le en brouillon jusqu’à ce que vous soyez prêt.' },
	route: SESSIONS,
	variants: { viewport: ['desktop', 'mobile'], theme: ['dark'] },
	level: 'checked',
	steps: [
		{
			id: 'open',
			route: SESSIONS,
			target: 'new',
			do: { kind: 'click' },
			say: {
				title: { en: 'Open the new-session dialog', fr: 'Ouvrir la fenêtre de nouvelle session' },
				body: { en: 'Everything a run needs is in this one dialog: the machine, the folder, the prompt and the profile.', fr: 'Tout ce dont un run a besoin tient dans cette fenêtre : la machine, le dossier, l’instruction et le profil.' }
			},
			expect: [{ visible: 'spawn' }, { visible: 'spawn/prompt' }],
			capture: 'dialog'
		},
		{
			id: 'where',
			route: SESSIONS,
			target: 'where',
			say: {
				title: { en: 'Pick where it runs', fr: 'Choisir où le run s’exécute' },
				body: { en: 'Choose the machine and the folder the agent should work in. The folder is the agent’s whole world — it reads and edits what is inside, so point it at the project you mean.', fr: 'Choisissez la machine et le dossier dans lequel l’agent doit travailler. Ce dossier est tout son univers : il y lit et modifie les fichiers, alors visez le bon projet.' }
			},
			expect: [{ visible: 'where' }]
		},
		{
			id: 'name',
			route: SESSIONS,
			target: 'spawn/label',
			do: { kind: 'fill', value: param('var.label') },
			say: {
				title: { en: 'Name the run', fr: 'Nommer le run' },
				body: { en: 'Give the run a name you will recognise in the list once a dozen of them are running.', fr: 'Donnez au run un nom que vous reconnaîtrez dans la liste quand une douzaine d’autres tourneront.' }
			}
		},
		{
			id: 'prompt',
			route: SESSIONS,
			target: 'spawn/prompt',
			do: { kind: 'fill', value: param('var.prompt') },
			say: {
				title: { en: 'Say what you want done', fr: 'Dire ce que vous voulez faire' },
				body: { en: 'Say what you want done. The profile below decides which harness and model carry it out.', fr: 'Dites ce que vous voulez faire. Le profil ci-dessous décide quel harnais et quel modèle s’en chargent.' }
			},
			capture: 'filled'
		},
		{
			id: 'profiles',
			route: SESSIONS,
			target: 'profiles',
			say: {
				title: { en: 'The profile decides how it thinks', fr: 'Le profil décide de sa façon de penser' },
				body: { en: 'A profile bundles the harness, model, reasoning effort and permission mode. Pick one here rather than setting four things every time you launch.', fr: 'Un profil regroupe le harnais, le modèle, l’effort de réflexion et le mode de permissions. Choisissez-en un ici plutôt que de régler quatre choses à chaque lancement.' }
			},
			expect: [{ visible: 'profiles' }],
			capture: 'profiles'
		},
		{
			id: 'profile-new',
			route: SESSIONS,
			target: 'new-profile',
			say: {
				title: { en: 'Save the combinations you reuse', fr: 'Enregistrer les combinaisons que vous réutilisez' },
				body: { en: 'Keep one profile for cheap throwaway work and another for the runs you want to be careful. Drag them into the order you reach for most.', fr: 'Gardez un profil pour le travail jetable et bon marché, un autre pour les runs qui demandent du soin. Réordonnez-les selon ce que vous utilisez le plus.' }
			},
			expect: [{ visible: 'new-profile' }]
		},
		{
			id: 'save',
			route: SESSIONS,
			// Lives in the Modal footer, outside the `spawn` body div: it must stay
			// a bare top-level path.
			target: 'draft',
			do: { kind: 'click' },
			say: {
				title: { en: 'Save it as a draft', fr: 'L’enregistrer comme brouillon' },
				body: { en: 'The Draft button lights up once the machine and folder are set. This saves the run on your instance without starting it — nothing executes until you launch it.', fr: 'Le bouton Brouillon s’active une fois la machine et le dossier renseignés. Le run est enregistré sur votre instance sans démarrer : rien ne s’exécute avant que vous ne le lanciez.' }
			},
			expect: [{ hidden: 'spawn' }, { probe: 'sessions.drafts' }],
			capture: 'saved'
		},
		{
			id: 'sections',
			route: SESSIONS,
			optional: true,
			target: 'sections/toggle',
			do: { kind: 'click' },
			say: {
				title: { en: 'Choose what the list shows', fr: 'Choisir ce qu’affiche la liste' },
				body: { en: 'The list is split into sections you can switch on and off; drafts are hidden until you ask for them.', fr: 'La liste est découpée en sections que vous pouvez activer ou désactiver ; les brouillons restent masqués jusqu’à ce que vous les demandiez.' }
			},
			expect: [{ visible: 'sections/option[drafts]' }]
		},
		{
			id: 'show-drafts',
			route: SESSIONS,
			optional: true,
			target: 'sections/option[drafts]',
			do: { kind: 'click' },
			say: {
				title: { en: 'The draft is waiting', fr: 'Le brouillon vous attend' },
				body: { en: 'Your draft is here, holding the machine, folder, profile and prompt until you launch it.', fr: 'Votre brouillon est ici : il conserve la machine, le dossier, le profil et l’instruction jusqu’au lancement.' }
			},
			expect: [{ count: ['section[drafts]/session', { min: 1 }] }, { probe: 'sessions.drafts' }],
			capture: 'draft'
		},
		{
			id: 'done',
			route: '/settings/guides',
			target: 'page[guides]',
			say: {
				title: { en: 'You can put an agent to work', fr: 'Vous savez mettre un agent au travail' },
				body: { en: 'You know what a run needs, what a profile carries, and that a draft costs nothing until you launch it. Next, learn to follow one while it works.', fr: 'Vous savez ce qu’exige un run, ce que porte un profil, et qu’un brouillon ne coûte rien tant qu’il n’est pas lancé. Ensuite, apprenez à suivre un run en cours.' }
			},
			expect: [{ visible: 'page[guides]' }]
		}
	]
});
