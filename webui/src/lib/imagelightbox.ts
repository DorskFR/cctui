import { browser } from '$app/environment';
import { installFileViewer } from '$lib/fileviewer';

// Delegated click-to-open-full for agent-posted images. Inline images
// are injected as raw HTML via {@html} in markdown bodies, so — like the
// code-copy buttons — one document-level listener covers every `.md-img`
// anywhere. Clicking an inline image opens a full-size overlay; clicking the
// overlay (or Escape) closes it. The full image is the same session-scoped,
// cookie-authed URL, so no extra fetch/credential handling is needed.
let installed = false;

export function installImageLightbox(): void {
	if (!browser || installed) return;
	installed = true;
	installFileViewer();
	document.addEventListener('click', (e) => {
		const target = e.target as HTMLElement | null;
		const img = target?.closest('.md-img') as HTMLImageElement | null;
		if (!img) return;
		const full = img.getAttribute('data-lightbox') ?? img.getAttribute('src');
		if (!full) return;
		e.preventDefault();
		e.stopPropagation();
		open(full);
	});
}

function open(src: string): void {
	const overlay = document.createElement('div');
	overlay.className = 'md-lightbox';
	const full = document.createElement('img');
	full.className = 'md-lightbox-img';
	full.src = src;
	overlay.appendChild(full);

	const close = (): void => {
		overlay.remove();
		document.removeEventListener('keydown', onKey);
	};
	const onKey = (ev: KeyboardEvent): void => {
		if (ev.key === 'Escape') close();
	};
	overlay.addEventListener('click', close);
	document.addEventListener('keydown', onKey);
	document.body.appendChild(overlay);
}
