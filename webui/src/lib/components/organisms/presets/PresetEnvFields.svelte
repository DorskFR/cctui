<script lang="ts">
	import { Button, Field, Icon } from '@dorsk/tsumikit';
	import EnvSecretsField from '#lib/components/organisms/spawn/EnvSecretsField.svelte';
	import { m } from '#lib/paraglide/messages.js';
	import type { PresetForm, PresetFormProblems } from '#lib/schedules/presetForm.ts';

	let {
		form = $bindable(),
		problems
	}: {
		form: PresetForm;
		problems: PresetFormProblems;
	} = $props();

	const addRow = () => (form.env = [...form.env, { key: '', value: '' }]);
</script>

<Field label={m.schedules_env_label()} hint={m.schedules_env_help()}>
	<div class="env" data-testid="preset-env">
		<EnvSecretsField bind:envRows={form.env} invalid={!!problems.env} />
		<span>
			<Button size="sm" onclick={addRow}><Icon name="plus" />{m.spawn_add_env_vars()}</Button>
		</span>
	</div>
</Field>

<style>
	.env {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
</style>
