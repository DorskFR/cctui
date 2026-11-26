<script lang="ts">
	import { goto } from '$app/navigation';
	import { ConfirmModal, NotificationCenter, Switch } from '@dorsk/tsumikit';
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
	import { notify } from '#lib/notify.svelte.ts';
	import { settings } from '#lib/settings.svelte.ts';
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

	let blockingId = $state<string | null>(null);
	let confirmingBlocking = $state(false);

	async function ondismiss(id: string) {
		const out = await store.dismiss(id);
		if (out === 'blocking') {
			blockingId = id;
			confirmingBlocking = true;
			return;
		}
		if (out !== 'dismissed') toasts.error(m.notif_dismiss_failed());
	}

	async function dismissBlocking() {
		const id = blockingId;
		if (!id) return;
		const out = await store.dismiss(id, true);
		if (out !== 'dismissed') toasts.error(m.notif_dismiss_failed());
	}

	function onreadall() {
		store.readAll().catch((e) => toasts.error(m.notif_load_failed({ error: errMessage(e) })));
	}

	async function toggleNotify() {
		if (settings.notifyEnabled) {
			await settings.setNotifyEnabled(false);
			toasts.info(m.nav_notify_off());
			return;
		}
		if (!notify.supported) {
			toasts.error(m.nav_notify_unsupported());
			return;
		}
		const ok = await settings.setNotifyEnabled(true);
		if (ok) toasts.ok(m.nav_notify_on());
		else toasts.error(m.nav_notify_blocked());
	}

	function onitemopen(id: string) {
		if (items.find((n) => n.id === id)?.href) open = false;
	}
</script>

{#snippet footer()}
	<Switch
		size="sm"
		label={m.notif_desktop_toggle()}
		labelVisible
		checked={settings.notifyEnabled}
		onclick={(e) => {
			e.preventDefault();
			void toggleNotify();
		}}
	/>
{/snippet}

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
	{footer}
/>

<ConfirmModal
	bind:open={confirmingBlocking}
	tone="danger"
	title={m.notif_dismiss_blocking_title()}
	message={m.notif_dismiss_blocking()}
	confirmLabel={m.notif_dismiss_anyway()}
	cancelLabel={m.common_cancel()}
	onconfirm={dismissBlocking}
/>
