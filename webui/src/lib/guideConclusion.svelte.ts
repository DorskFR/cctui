import { mount, unmount } from 'svelte';
import GuideConclusion from './components/organisms/GuideConclusion.svelte';

export interface Conclusion {
	title: string;
	xp: number;
}

/** Own presenter rather than the runtime's: the engine has already hidden its
 *  overlay and cleared the run by the time a tour is over. Resolves when the
 *  user is done reading, so the caller can take them back to the guides. */
export function showConclusion(conclusion: Conclusion): Promise<void> {
	if (typeof document === 'undefined') return Promise.resolve();
	return new Promise((done) => {
		let card: Record<string, unknown> | null = null;
		const close = () => {
			if (!card) return;
			unmount(card, { outro: false });
			card = null;
			done();
		};
		card = mount(GuideConclusion, {
			target: document.body,
			props: { title: conclusion.title, xp: conclusion.xp, ondone: close }
		}) as Record<string, unknown>;
	});
}
