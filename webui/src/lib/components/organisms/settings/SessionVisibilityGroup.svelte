<script lang="ts">
	import type { HiddenOwnerKind } from '@bindings/HiddenOwnerKind';
	import { Badge, Switch } from '@dorsk/tsumikit';
	import SettingGroup from '#lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '#lib/components/molecules/SettingRow.svelte';
	import {
		useAllMachines,
		useHiddenOwnerActions,
		useHiddenOwners,
		useMe,
		useUsers
	} from '#lib/queries/index.ts';
	import { errMessage } from '#lib/api.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import { setShown, visibilityRows } from './sessionVisibility.logic';
	import { m } from '#lib/paraglide/messages.js';

	let { isAdmin = false }: { isAdmin?: boolean } = $props();

	const me = useMe();
	const users = useUsers(() => isAdmin);
	const machines = useAllMachines(() => isAdmin);
	const owners = useHiddenOwners(() => isAdmin);
	const actions = useHiddenOwnerActions();
	const rows = $derived(
		visibilityRows(users.data, machines.data, owners.data?.owners, me.data?.user_id)
	);
	let busy = $state<string | null>(null);

	async function toggle(kind: HiddenOwnerKind, id: string, name: string, shown: boolean) {
		busy = `${kind}:${id}`;
		try {
			await setShown(actions, kind, id, name, shown);
			toasts.ok(shown ? m.hidden_owners_unhidden_toast({ name }) : m.hidden_owners_hidden_toast({ name }));
		} catch (e) {
			toasts.error(errMessage(e));
		} finally {
			busy = null;
		}
	}
</script>

{#snippet userHidden()}
	{m.settings_visibility_user_help()}
	<Badge tone="muted" size="xs" data-journey="hidden-chip">{m.hidden_owners_chip()}</Badge>
{/snippet}
{#snippet machineHidden()}
	{m.settings_visibility_machine_help()}
	<Badge tone="muted" size="xs" data-journey="hidden-chip">{m.hidden_owners_chip()}</Badge>
{/snippet}

{#if isAdmin}
	<SettingGroup title={m.settings_visibility_title()}>
		{#each rows as u (u.id)}
			{#if u.shown !== null}
				<SettingRow
					label={u.name}
					help={m.settings_visibility_user_help()}
					helpSlot={u.shown ? undefined : userHidden}
					journey="visibility-user"
					journeyKey={u.id}
				>
					<Switch
						bind:checked={() => u.shown === true, (v) => void toggle('user', u.id, u.name, v)}
						disabled={busy === `user:${u.id}`}
						label={m.settings_visibility_switch_named({ name: u.name })}
						data-visibility="user"
						data-target={u.id}
					/>
				</SettingRow>
			{/if}
			{#each u.machines as mc (mc.id)}
				<SettingRow
					label={`${u.name} › ${mc.name}`}
					help={m.settings_visibility_machine_help()}
					helpSlot={mc.shown ? undefined : machineHidden}
					journey="visibility-machine"
					journeyKey={mc.id}
				>
					<Switch
						bind:checked={() => mc.shown, (v) => void toggle('machine', mc.id, mc.name, v)}
						disabled={busy === `machine:${mc.id}` || u.shown === false}
						label={m.settings_visibility_switch_named({ name: mc.name })}
						data-visibility="machine"
						data-target={mc.id}
					/>
				</SettingRow>
			{/each}
		{/each}
	</SettingGroup>
{/if}
