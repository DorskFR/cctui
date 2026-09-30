<script lang="ts">
	import { Switch } from '@dorsk/tsumikit';
	import SettingGroup from '$lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '$lib/components/molecules/SettingRow.svelte';
	import { auth } from '$lib/auth.svelte';
	import { endpoints } from '$lib/queries';
	import { toasts } from '$lib/toast.svelte';
	import type { PasskeyConfig } from '@bindings/PasskeyConfig';
	import { m } from '$lib/paraglide/messages';

	let cfg = $state<PasskeyConfig | null>(null);
	$effect(() => {
		auth
			.passkeyConfig()
			.then((c) => (cfg = c))
			.catch(() => {});
	});

	async function setAutoPrompt(on: boolean) {
		try {
			await endpoints.setPasskeyAutoPrompt(on);
			if (cfg) cfg = { ...cfg, auto_prompt: on };
		} catch (e) {
			toasts.error(e instanceof Error ? e.message : String(e));
		}
	}
</script>

{#if cfg?.available}
	<SettingGroup title={m.settings_signin_group()}>
		<SettingRow
			label={m.settings_passkeys_auto_prompt_label()}
			help={m.settings_passkeys_auto_prompt_help()}
			server
			admin
		>
			<Switch
				bind:checked={() => cfg?.auto_prompt === true, (v) => setAutoPrompt(v)}
				label={m.settings_passkeys_auto_prompt_label()}
			/>
		</SettingRow>
	</SettingGroup>
{/if}
