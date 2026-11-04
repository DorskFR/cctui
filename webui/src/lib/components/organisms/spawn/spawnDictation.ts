import { tick } from 'svelte';
import { insertBlock } from '../conversation/insertText';
import type { SpawnForm } from './spawnForm.svelte';

export type DictationTarget = Pick<SpawnForm, 'form' | 'promptEl' | 'submit'>;

/** A finished dictation lands in the prompt at the caret; with auto-send on
 *  it spawns straight away. */
export async function dictateIntoPrompt(sf: DictationTarget, text: string, autoSend: boolean): Promise<void> {
	const el = sf.promptEl;
	const caret = el && document.activeElement === el ? (el.selectionStart ?? undefined) : undefined;
	const next = insertBlock(sf.form.prompt, text, caret);
	sf.form.prompt = next.value;
	if (autoSend) {
		await sf.submit();
		return;
	}
	await tick();
	if (!el) return;
	el.focus();
	el.setSelectionRange(next.caret, next.caret);
}
