import { browser } from '$app/environment';
import { apiBlob } from '$lib/api';
import { m } from '$lib/paraglide/messages';
import { renderMarkdown } from '$lib/markdown';
import { toasts } from '$lib/toast.svelte';

// Delegated opener for agent-linked local files (`a.md-file`, injected via
// {@html} by the markdown renderer, so — like the image lightbox — one
// document-level listener covers every bubble). The link points at the
// machine-scoped read-file route; the response's content type decides what
// happens: images and text/markdown open in an overlay, anything else is
// downloaded. Refusals (too large, outside the allow-list, daemon offline)
// surface next to the link instead of navigating the tab to the API's body.
let installed = false;

export function installFileViewer(): void {
	if (!browser || installed) return;
	installed = true;
	document.addEventListener('click', (e) => {
		if (e.defaultPrevented || e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey) return;
		const target = e.target as HTMLElement | null;
		const link = target?.closest('a.md-file') as HTMLAnchorElement | null;
		if (!link) return;
		const href = link.getAttribute('href');
		if (!href) return;
		e.preventDefault();
		e.stopPropagation();
		const name = link.dataset.fileName ?? link.textContent ?? 'file';
		void attemptOpen(href, name).then((refusal) => {
			const text = refusal
				? refusalMessage(refusal.status, name, 'machine', refusal.detail)
				: null;
			showInlineRefusal(link, text);
		});
	});
}

/** Put `text` in the link's own refusal slot, or clear it when `text` is null,
 * so a retry that succeeds takes the message away with it. */
function showInlineRefusal(link: HTMLAnchorElement, text: string | null): void {
	const existing = link.nextElementSibling;
	if (existing?.classList.contains('md-file-error')) existing.remove();
	if (!text) return;
	const span = document.createElement('span');
	span.className = 'md-file-error';
	span.setAttribute('role', 'status');
	span.textContent = text;
	link.after(span);
}

/** A read the route refused: the HTTP status (`0` for a network failure) and
 * the server's `{"error": …}` body, which for a denial names the roots the
 * path was checked against. */
export interface Refusal {
	status: number;
	detail: string;
	/** The route's structured `allowed_folders`; empty from an older server. */
	allowedFolders?: string[];
}

/** The roots a denial was checked against. The structured list wins; the prose
 *  parse is the fallback for a server or daemon that does not send one. */
export function deniedRoots(detail: string, allowedFolders: string[] = []): string[] {
	if (allowedFolders.length) return allowedFolders;
	const at = detail.indexOf('allowed roots:');
	if (at < 0) return [];
	return detail
		.slice(at + 'allowed roots:'.length)
		.split(',')
		.map((r) => r.trim())
		.filter(Boolean);
}

/** The session and machine that linked a path, as `linked-file-owner` reports. */
export interface LinkedFileOwner {
	session_id: string;
	machine_id: string;
}

/** The same read aimed at the machine that owns the link. `null` when `href`
 *  is not a machine file href, so a blob href is never rewritten. */
export function retargetHref(href: string, owner: LinkedFileOwner): string | null {
	const url = new URL(href, 'http://cctui.invalid');
	const path = url.pathname.replace(
		/\/machines\/[^/]+\/fs\/file$/,
		`/machines/${encodeURIComponent(owner.machine_id)}/fs/file`
	);
	if (path === url.pathname) return null;
	url.pathname = path;
	url.searchParams.set('session_id', owner.session_id);
	return url.pathname + url.search;
}

/** Which session and machine linked `path`, or `null` when none the viewer can
 *  read did. */
async function linkOwner(sessionId: string, path: string): Promise<LinkedFileOwner | null> {
	const url = `/api/v1/sessions/${encodeURIComponent(sessionId)}/linked-file-owner?path=${encodeURIComponent(path)}`;
	try {
		const res = await apiBlob(url);
		if (!res.ok) return null;
		const owner = (await res.json()) as LinkedFileOwner;
		return owner.machine_id && owner.session_id ? owner : null;
	} catch {
		return null;
	}
}

/** A refusal the owning machine might not give: the path may simply belong to
 *  another machine's session. */
function mayLiveElsewhere(status: number): boolean {
	return status === 403 || status === 404;
}

export type FileKind = 'image' | 'text' | 'markdown' | 'download';

/** What the viewer does with a response of this content type. */
export function classify(contentType: string | null): FileKind {
	const base = (contentType ?? '').split(';')[0].trim().toLowerCase();
	if (base.startsWith('image/')) return 'image';
	if (base === 'text/markdown') return 'markdown';
	if (base === 'text/plain' || base === 'application/json') return 'text';
	return 'download';
}

/**
 * Which route the href points at. `blob` is the server's own store
 * (`/sessions/{id}/blobs/{hash}`), which knows nothing about any machine, so
 * its refusals must never be worded as a machine-side absence; `machine` is
 * `/machines/{id}/fs/file`, where the daemon and the filesystem are in play.
 */
export type FileSource = 'machine' | 'blob';

/** User-facing text for a refused read, by HTTP status and route. */
export function refusalMessage(
	status: number,
	name: string,
	source: FileSource = 'machine',
	detail = '',
	allowedFolders: string[] = []
): string {
	if (status === 0) return m.conversation_file_open_failed({ name, status: 'network' });
	if (source === 'blob') {
		return status === 404
			? m.conversation_attachment_gone({ name })
			: m.conversation_file_open_failed({ name, status: String(status) });
	}
	switch (status) {
		case 413:
			return m.conversation_file_too_large({ name });
		case 403: {
			const roots = deniedRoots(detail, allowedFolders);
			return roots.length
				? m.conversation_file_denied_roots({ name, roots: roots.join(', ') })
				: m.conversation_file_denied({ name });
		}
		case 404:
			return m.conversation_file_not_found({ name });
		case 503:
		case 504:
			return m.conversation_file_daemon_offline({ name });
		default:
			return m.conversation_file_open_failed({ name, status: String(status) });
	}
}

/**
 * [`attemptOpen`] reduced to the status, for a caller with a fallback chain
 * that only needs to know whether to try the next source.
 */
export async function tryOpenLocalFile(
	href: string,
	name: string
): Promise<number | null> {
	return (await attemptOpen(href, name))?.status ?? null;
}

/** Open `href`, returning `null` on success and the [`Refusal`] otherwise —
 * without surfacing anything, so the caller decides between a fallback source,
 * a toast and an inline message. */
export async function attemptOpen(href: string, name: string): Promise<Refusal | null> {
	const first = await readOnce(href, name);
	if (!first || !mayLiveElsewhere(first.status)) return first;
	const retried = await retryOnOwningMachine(href, name);
	return retried === undefined ? first : retried;
}

/** Re-ask the machine that linked the path. `undefined` means there was nobody
 *  else to ask, so the original refusal stands. */
async function retryOnOwningMachine(
	href: string,
	name: string
): Promise<Refusal | null | undefined> {
	const url = new URL(href, 'http://cctui.invalid');
	const path = url.searchParams.get('path');
	const sessionId = url.searchParams.get('session_id');
	if (!path || !sessionId) return undefined;
	const owner = await linkOwner(sessionId, path);
	if (!owner) return undefined;
	const next = retargetHref(href, owner);
	return next ? await readOnce(next, name) : undefined;
}

async function readOnce(href: string, name: string): Promise<Refusal | null> {
	let res: Response;
	try {
		res = await apiBlob(href);
	} catch {
		return { status: 0, detail: '' };
	}
	if (!res.ok) return { status: res.status, ...(await errorDetail(res)) };
	await present(res, name);
	return null;
}

async function errorDetail(res: Response): Promise<{ detail: string; allowedFolders: string[] }> {
	try {
		const body = (await res.json()) as { error?: unknown; allowed_folders?: unknown };
		const folders = Array.isArray(body.allowed_folders)
			? body.allowed_folders.filter((f): f is string => typeof f === 'string')
			: [];
		return { detail: typeof body.error === 'string' ? body.error : '', allowedFolders: folders };
	} catch {
		return { detail: '', allowedFolders: [] };
	}
}

export async function openLocalFile(
	href: string,
	name: string,
	source: FileSource = 'machine'
): Promise<void> {
	const refusal = await attemptOpen(href, name);
	if (refusal)
		toasts.error(
			refusalMessage(refusal.status, name, source, refusal.detail, refusal.allowedFolders)
		);
}

async function present(res: Response, name: string): Promise<void> {
	const kind = classify(res.headers.get('content-type'));
	if (kind === 'download') {
		download(await res.blob(), name);
		return;
	}
	if (kind === 'image') {
		const url = URL.createObjectURL(await res.blob());
		openOverlay(name, url, () => URL.revokeObjectURL(url), (body) => {
			const img = document.createElement('img');
			img.className = 'md-lightbox-img';
			img.src = url;
			img.alt = name;
			body.appendChild(img);
		});
		return;
	}
	const text = await res.text();
	const blobUrl = URL.createObjectURL(new Blob([text], { type: 'text/plain' }));
	openOverlay(name, blobUrl, () => URL.revokeObjectURL(blobUrl), (body) => {
		if (kind === 'markdown') {
			const div = document.createElement('div');
			div.className = 'md-fileviewer-md';
			div.innerHTML = renderMarkdown(text);
			body.appendChild(div);
		} else {
			const pre = document.createElement('pre');
			pre.className = 'md-fileviewer-pre';
			pre.textContent = text;
			body.appendChild(pre);
		}
	});
}

function download(blob: Blob, name: string): void {
	const url = URL.createObjectURL(blob);
	const a = document.createElement('a');
	a.href = url;
	a.download = name;
	document.body.appendChild(a);
	a.click();
	a.remove();
	setTimeout(() => URL.revokeObjectURL(url), 10_000);
}

function openOverlay(
	name: string,
	downloadUrl: string,
	cleanup: () => void,
	fill: (body: HTMLElement) => void
): void {
	const previous = document.activeElement as HTMLElement | null;
	const overlay = document.createElement('div');
	overlay.className = 'md-lightbox md-fileviewer';
	overlay.setAttribute('role', 'dialog');
	overlay.setAttribute('aria-modal', 'true');
	overlay.setAttribute('aria-label', name);

	const panel = document.createElement('div');
	panel.className = 'md-fileviewer-panel';
	panel.addEventListener('click', (e) => e.stopPropagation());

	const head = document.createElement('div');
	head.className = 'md-fileviewer-head';
	const title = document.createElement('span');
	title.className = 'md-fileviewer-title';
	title.textContent = name;
	const dl = document.createElement('a');
	dl.className = 'md-fileviewer-btn';
	dl.href = downloadUrl;
	dl.download = name;
	dl.textContent = m.conversation_file_download();
	const closeBtn = document.createElement('button');
	closeBtn.type = 'button';
	closeBtn.className = 'md-fileviewer-btn';
	closeBtn.textContent = m.common_close();
	closeBtn.setAttribute('aria-label', m.common_close());
	head.append(title, dl, closeBtn);

	const body = document.createElement('div');
	body.className = 'md-fileviewer-body';
	fill(body);
	panel.append(head, body);
	overlay.appendChild(panel);

	const close = (): void => {
		overlay.remove();
		document.removeEventListener('keydown', onKey);
		cleanup();
		previous?.focus?.();
	};
	const onKey = (ev: KeyboardEvent): void => {
		if (ev.key === 'Escape') close();
	};
	closeBtn.addEventListener('click', close);
	overlay.addEventListener('click', close);
	document.addEventListener('keydown', onKey);
	document.body.appendChild(overlay);
	closeBtn.focus();
}
