const FALLBACK_TITLE = 'cctui';
const RESOLVED_TITLE = 'Resolved';

// WebKit revokes a subscription whose pushes show nothing, so a retract there
// leaves a quiet "resolved" line in place of the closed notice.
function mustShowEveryPush() {
	const ua = self.navigator.userAgent;
	return /AppleWebKit/.test(ua) && !/Chrome\/|Chromium\//.test(ua);
}

function safeUrl(raw) {
	try {
		const url = new URL(raw || '/', self.location.origin);
		return url.protocol === 'https:' || url.protocol === 'http:' ? url.href : null;
	} catch {
		return null;
	}
}

function parse(event) {
	try {
		return event.data ? event.data.json() : null;
	} catch {
		return null;
	}
}

function show(msg) {
	const actions = Array.isArray(msg.actions) ? msg.actions : [];
	const links = {};
	for (const a of actions) links[a.action] = a.url;
	return self.registration.showNotification(msg.title || FALLBACK_TITLE, {
		body: msg.session || undefined,
		tag: msg.tag || undefined,
		renotify: Boolean(msg.renotify && msg.tag),
		data: { url: msg.data && msg.data.url, actions: links },
		actions: actions.map((a) => ({ action: a.action, title: a.title }))
	});
}

async function close(msg) {
	const shown = await self.registration.getNotifications({ tag: msg.tag });
	for (const n of shown) n.close();
	if (mustShowEveryPush()) {
		await self.registration.showNotification(msg.title || RESOLVED_TITLE, {
			tag: msg.tag,
			silent: true
		});
	}
}

async function focusOrOpen(raw) {
	const url = safeUrl(raw) || safeUrl('/');
	const windows = await self.clients.matchAll({ type: 'window', includeUncontrolled: true });
	const open = windows.find((c) => c.url === url);
	if (open) return open.focus();
	return self.clients.openWindow(url);
}

self.addEventListener('install', () => self.skipWaiting());
self.addEventListener('activate', (event) => event.waitUntil(self.clients.claim()));

self.addEventListener('push', (event) => {
	const msg = parse(event);
	if (!msg) return;
	event.waitUntil(msg.type === 'close' ? close(msg) : show(msg));
});

self.addEventListener('notificationclick', (event) => {
	event.notification.close();
	const data = event.notification.data || {};
	const link = event.action && data.actions ? data.actions[event.action] : null;
	event.waitUntil(focusOrOpen(link || data.url));
});
