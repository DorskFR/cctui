import { defineJourney, param } from '@dorsk/journey';

/**
 * While the spawn dialog is open, every step must act on a control *inside* it.
 * The dialog is opened with `showModal()`, which makes everything outside it
 * non-interactive — including the guide's own card, which is still painted on
 * top but whose Next button the browser hit-tests straight through to the dialog
 * beneath. A passive step here strands the user on a button that does nothing.
 */
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
			target: 'where',
			do: { kind: 'click' },
			say: {
				title: { en: 'Pick where it runs', fr: 'Choisir où le run s’exécute' },
				body: { en: 'Choose the machine, then the folder. That folder is the agent’s whole world — it reads and edits what is inside it and nothing else, so point it at the project you actually mean. Set them now.', fr: 'Choisissez la machine, puis le dossier. Ce dossier est tout l’univers de l’agent — il y lit et modifie les fichiers, et rien d’autre : visez donc le bon projet. Réglez-les maintenant.' }
			},
			capture: 'where'
		},
		{
			id: 'name',
			target: 'spawn/label',
			do: { kind: 'fill', value: param('var.label') },
			say: {
				title: { en: 'Name the run', fr: 'Nommer le run' },
				body: { en: 'Give it a name you will recognise in the list once a dozen of them are running. Anything you type here will do.', fr: 'Donnez-lui un nom que vous reconnaîtrez dans la liste quand une douzaine d’autres tourneront. N’importe quel texte fera l’affaire.' }
			}
		},
		{
			id: 'prompt',
			target: 'spawn/prompt',
			do: { kind: 'fill', value: param('var.prompt') },
			say: {
				title: { en: 'Say what you want done', fr: 'Dire ce que vous voulez faire' },
				body: { en: 'This is the agent’s brief, and it is the whole instruction — it will not ask what you meant before it starts. Be as specific as you would be with a new colleague.', fr: 'C’est la consigne de l’agent, et c’est toute l’instruction — il ne demandera pas ce que vous vouliez dire avant de commencer. Soyez aussi précis qu’avec un nouveau collègue.' }
			},
			capture: 'filled'
		},
		{
			id: 'profiles',
			target: 'profiles',
			do: { kind: 'click' },
			say: {
				title: { en: 'The profile decides how it thinks', fr: 'Le profil décide de sa façon de penser' },
				body: { en: 'A profile bundles four things: the harness, the model, how hard it reasons, and how much it may do without asking. Pick one rather than setting all four every launch — keep a cheap one for throwaway work and a careful one for the rest.', fr: 'Un profil regroupe quatre choses : le harnais, le modèle, l’intensité du raisonnement et ce qu’il peut faire sans demander. Choisissez-en un plutôt que de régler les quatre à chaque lancement — gardez-en un bon marché pour le jetable et un prudent pour le reste.' }
			},
			expect: [{ visible: 'profiles' }],
			capture: 'profiles'
		},
		{
			id: 'save',
			// In the Modal footer, outside the `spawn` body div: a bare top-level path.
			target: 'draft',
			do: { kind: 'click' },
			say: {
				title: { en: 'Draft it rather than launch it', fr: 'L’enregistrer plutôt que le lancer' },
				body: { en: 'Launch would start the agent now — it would begin reading and editing the folder you chose. Draft, beside it, saves all of this and runs nothing. Press Draft: you can launch it whenever you like.', fr: 'Lancer démarrerait l’agent maintenant — il commencerait à lire et modifier le dossier choisi. Brouillon, à côté, enregistre tout et n’exécute rien. Appuyez sur Brouillon : vous pourrez le lancer quand vous voudrez.' }
			},
			expect: [{ hidden: 'spawn' }],
			capture: 'saved'
		},
		{
			id: 'drafts',
			target: 'sections',
			say: {
				title: { en: 'Where the draft went', fr: 'Où est passé le brouillon' },
				body: { en: 'Drafts have their own group in the list, switched off until you ask for it — behind this button, with all the other groups. Your run waits there with its machine, folder, profile and prompt intact, and nothing executes until you launch it.', fr: 'Les brouillons ont leur propre groupe dans la liste, désactivé jusqu’à ce que vous le demandiez — derrière ce bouton, avec les autres groupes. Votre run y attend avec sa machine, son dossier, son profil et son instruction intacts, et rien ne s’exécute avant le lancement.' }
			},
			expect: [{ visible: 'sections' }],
			capture: 'draft'
		}
	]
});
