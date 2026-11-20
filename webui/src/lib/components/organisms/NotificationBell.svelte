<script lang="ts">
	import { goto } from '$app/navigation';
	import { NotificationCenter } from '@dorsk/tsumikit';
	import { errMessage } from '#lib/api.ts';
	import {
		DETAILS,
		REDISPATCH,
		failedDispatchOf,
		failureHref,
		notifications,
		stateLabel,
		toItem,
		type NotificationStore
	} from '#lib/notifications.svelte.ts';
	import { failedDispatchEndpoints } from '#lib/queries/failedDispatches.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import { m } from '#lib/paraglide/messages.js';

	let {
		store = notifications,
		open = $bindable(false),
		redispatch = failedDispatchEndpoints.redispatch
	}: {
		store?: NotificationStore;
		open?: boolean;
		redispatch?: (record: string) => Promise<unknown>;
	} = $props();

	const items = $derived(store.items.map(toItem));

	function refresh() {
		store.load().catch((e) => toasts.error(m.notif_load_failed({ error: errMessage(e) })));
	}

	/** A hidden tab renders nothing anyone saw; reading would cancel the push. */
	function onvisible(id: string) {
		if (document.visibilityState === 'visible') void store.markRead(id);
	}

	async function rerun(record: string) {
		try {
			await redispatch(record);
			toasts.ok(m.notif_redispatched());
		} catch (e) {
			toasts.error(m.notif_act_failed({ error: errMessage(e) }));
		}
	}

	async function onaction(id: string, action: string) {
		const notice = store.items.find((n) => n.id === id);
		const failed = notice ? failedDispatchOf(notice) : null;
		if (failed && action === DETAILS) {
			open = false;
			return goto(failureHref(failed));
		}
		if (failed && action === REDISPATCH) return rerun(failed);
		const out = await store.act(id, action);
		if (out.kind === 'elsewhere') {
			const state = stateLabel(out.notification) ?? out.notification.state;
			toasts.info(m.notif_answered_elsewhere({ state }));
		} else if (out.kind === 'failed') {
			toasts.error(m.notif_act_failed({ error: errMessage(out.error) }));
		}
	}

	async function ondismiss(id: string) {
		let out = await store.dismiss(id);
		if (out === 'blocking') {
			if (!confirm(m.notif_dismiss_blocking())) return;
			out = await store.dismiss(id, true);
		}
		if (out !== 'dismissed') toasts.error(m.notif_dismiss_failed());
	}

	function onreadall() {
		store.readAll().catch((e) => toasts.error(m.notif_load_failed({ error: errMessage(e) })));
	}

	function onitemopen(id: string) {
		if (items.find((n) => n.id === id)?.href) open = false;
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
