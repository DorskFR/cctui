import type { MenuItem } from '@dorsk/tsumikit';
import type { MeResponse } from '@bindings/MeResponse';
import type { SessionListItem } from '@bindings/SessionListItem';
import { errMessage } from '$lib/api';
import { type OwnerTarget, sessionOwnerTargets } from '$lib/hiddenOwners';
import { m } from '$lib/paraglide/messages';
import { useAllMachines, useHiddenOwnerActions, useHiddenOwners, useUsers } from '$lib/queries';
import { toasts } from '$lib/toast.svelte';
import { hasFeature } from '$lib/features.svelte';

export function ownerTargetLabel(t: OwnerTarget): string {
	if (t.kind === 'user')
		return t.hidden ? m.hidden_owners_unhide_user({ name: t.label }) : m.hidden_owners_hide_user({ name: t.label });
	return t.hidden
		? m.hidden_owners_unhide_machine({ name: t.label })
		: m.hidden_owners_hide_machine({ name: t.label });
}

export async function toggleOwnerTarget(
	t: OwnerTarget,
	actions: Pick<ReturnType<typeof useHiddenOwnerActions>, 'hide' | 'unhide'>
) {
	try {
		if (t.hidden) {
			await actions.unhide(t.kind, t.target_id);
			toasts.ok(m.hidden_owners_unhidden_toast({ name: t.label }));
		} else {
			await actions.hide(t.kind, t.target_id, t.label);
			toasts.ok(m.hidden_owners_hidden_toast({ name: t.label }));
		}
	} catch (e) {
		toasts.error(errMessage(e));
	}
}

export function useOwnerHideMenu(session: () => SessionListItem, me: () => MeResponse | undefined) {
	const admin = () => me()?.role === 'admin';
	const owners = useHiddenOwners(admin);
	const machines = useAllMachines(() => admin() && hasFeature('machine_spawn'));
	const users = useUsers(admin);
	const actions = useHiddenOwnerActions();
	const items = $derived<MenuItem[]>(
		admin()
			? sessionOwnerTargets(session(), {
					meId: me()?.user_id,
					machines: machines.data,
					users: users.data,
					owners: owners.data?.owners
				}).map((t) => ({
					label: ownerTargetLabel(t),
					icon: t.hidden ? ('eye' as const) : ('eye-off' as const),
					onselect: () => void toggleOwnerTarget(t, actions)
				}))
			: []
	);
	return {
		get items() {
			return items;
		}
	};
}
