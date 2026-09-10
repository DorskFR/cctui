<script lang="ts">
	import { errMessage } from '$lib/api';
	import { useAccountActions, type OAuthAccount, type UpdateAccount } from '$lib/queries';
	import { toasts } from '$lib/toast.svelte';
	import FreeFormEnvEditor from '$lib/components/organisms/FreeFormEnvEditor.svelte';
	import { envObject } from '../account-editor.logic';
	import { Button, Card } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	let { account }: { account: OAuthAccount } = $props();

	const actions = useAccountActions();
	let envRows = $state<{ name: string; value: string }[]>([]);
	let replaceEnv = $state(false);
	let envRemove = $state<string[]>([]);
	let saving = $state(false);

	const dirty = $derived(replaceEnv || envRemove.length > 0);

	async function save() {
		saving = true;
		try {
			const body: UpdateAccount = {};
			if (replaceEnv) body.env_json = envObject(envRows);
			else if (envRemove.length) body.env_remove = envRemove;
			await actions.update(account.id, body);
			toasts.ok(m.accounts_account_updated());
			envRows = [];
			replaceEnv = false;
			envRemove = [];
		} catch (e) {
			toasts.error(errMessage(e));
		} finally {
			saving = false;
		}
	}
</script>

<Card title={m.account_section_env()} subtitle={m.account_section_env_help()}>
	{#snippet actions()}
		<Button size="sm" variant="primary" disabled={!dirty || saving} onclick={save}>
			{m.common_save()}
		</Button>
	{/snippet}

	<FreeFormEnvEditor
		bind:envRows
		bind:replaceEnv
		bind:envRemove
		storedNames={account.env_names ?? []}
	/>
</Card>
