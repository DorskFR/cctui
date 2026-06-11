import { browser } from '$app/environment';

// Single delegated click handler for the per-code-block copy buttons (CCT-297
// #20). Code blocks are injected as raw HTML via {@html} (markdown bodies,
// tool-call/result panes), so we can't bind Svelte handlers per block; one
// document-level listener covers every `.md-copy` button anywhere in the app.
let installed = false;

export function installCodeCopy(): void {
	if (!browser || installed) return;
	installed = true;
	document.addEventListener('click', (e) => {
		const target = e.target as HTMLElement | null;
		const btn = target?.closest('.md-copy') as HTMLButtonElement | null;
		if (!btn) return;
		e.preventDefault();
		e.stopPropagation();
		const code = btn.closest('.md-pre-wrap')?.querySelector('code');
		const text = code?.textContent ?? '';
		if (!text) return;
		navigator.clipboard
			.writeText(text)
			.then(() => flash(btn, 'Copied'))
			.catch(() => flash(btn, 'Failed'));
	});
}

function flash(btn: HTMLButtonElement, label: string): void {
	const prev = btn.textContent;
	btn.textContent = label;
	btn.classList.add('done');
	window.setTimeout(() => {
		btn.textContent = prev;
		btn.classList.remove('done');
	}, 1200);
}
