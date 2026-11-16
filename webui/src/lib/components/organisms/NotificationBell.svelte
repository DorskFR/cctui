<script lang="ts">
	import { goto } from '$app/navigation';
	import { NotificationCenter } from '@dorsk/tsumikit';
	import { errMessage } from '$lib/api';
	import {
		notifications,
		stateLabel,
		toItem,
		type NotificationStore
	} from '$lib/notifications.svelte';
	import { notify } from '$lib/notify.svelte';
	import { toasts } from '$lib/toast.svelte';
	import { m } from '$lib/paraglide/messages';

	let {
		store = notifications,
		open = $bindable(false)
	}: { store?: NotificationStore; open?: boolean } = $props();

	const items = $derived(store.items.map(toItem));

	function refresh() {
		store.load().catch((e) => toasts.error(m.notif_load_failed({ error: errMessage(e) })));
	}

	/** A hidden tab renders nothing anyone saw; reading would cancel the push. */
	function onvisible(id: string) {
		if (document.visibilityState === 'visible') void store.markRead(id);
	}

	async function onaction(id: string, action: string) {
		const out = await store.act(id, action);
		if (out.kind === 'elsewhere') {
			const state = stateLabel(out.notification) ?? out.notification.state;
			toasts.info(m.notif_answered_elsewhere({ state }));
		} else if (out.kind === 'failed') {
			toasts.error(m.notif_act_failed({ error: errMessage(out.error) }));
		}
	}

	async function ondismiss(id: string) {
		const out = await store.dismiss(id);
		if (out === 'blocking') toasts.info(m.notif_dismiss_blocking());
		else if (out === 'failed') toasts.error(m.notif_dismiss_failed());
	}

	function onreadall() {
		store.readAll().catch((e) => toasts.error(m.notif_load_failed({ error: errMessage(e) })));
	}

	function onitemopen(id: string) {
		const sessionId = store.items.find((n) => n.id === id)?.session_id;
		if (!sessionId) return;
		open = false;
		notify.pendingOpen = sessionId;
		if (!/^\/sessions(\/|$)/.test(location.pathname)) void goto('/sessions');
	}
</script>

<NotificationCenter
	{items}
	unread={store.unread}
	loading={store.loading}
	hasMore={store.hasMore}
	bind:open
	title={m.notif_center_title()}
	markAllLabel={m.notif_center_mark_all()}
	closeLabel={m.notif_center_close()}
	emptyTitle={m.notif_center_empty_title()}
	emptyDescription={m.notif_center_empty_description()}
	placement="bottom-end"
	onopenchange={(o) => {
		if (o) refresh();
	}}
	{onaction}
	onread={(id) => void store.markRead(id)}
	{onreadall}
	onloadmore={() => void store.loadMore().catch(() => {})}
	{onvisible}
	{onitemopen}
	{ondismiss}
/>
