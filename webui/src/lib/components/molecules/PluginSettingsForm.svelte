<script lang="ts">
	// The settings a plugin declares in its manifest, one text field each,
	// written to the user's `plugins.config[<id>]` on blur / Enter.
	import { Field, Input, Text } from '@dorsk/tsumikit';
	import type { PluginSettingDecl } from '$lib/plugins/types';
	import { m } from '$lib/paraglide/messages';

	let {
		pluginId,
		decls,
		values,
		onchange
	}: {
		pluginId: string;
		decls: PluginSettingDecl[];
		values: Record<string, string>;
		onchange: (key: string, value: string) => void;
	} = $props();
</script>

{#if decls.length}
	<div class="form" data-journey="plugin-config" data-plugin={pluginId}>
		{#each decls as d (d.key)}
			<Field label={d.label} hint={m.settings_plugins_config_env({ env: d.env })}>
				<Input
					value={values[d.key] ?? ''}
					autocomplete="off"
					spellcheck={false}
					data-journey="plugin-config-field"
					data-key={d.key}
					onblur={(e) => onchange(d.key, (e.currentTarget as HTMLInputElement).value.trim())}
					onenter={(v) => onchange(d.key, v.trim())}
				/>
			</Field>
		{/each}
		<Text size="xs" tone="faint">{m.settings_plugins_config_private()}</Text>
	</div>
{/if}

<style>
	.form {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		padding: 0 var(--sp-3) var(--sp-3);
	}
</style>
