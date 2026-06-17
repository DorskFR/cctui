<script lang="ts">
	// Global app settings (CCT-???). Device-local preference toggles backed by the
	// `settings` singleton (localStorage). Mirrors the Users page layout: a Card
	// with a <dl> of label/description on the left and a Switch on the right.
	import { settings, SETTING_DEFS } from '$lib/settings.svelte';
	import { Card, Heading, Stack, Switch, Text } from '@dorsk/tsumikit';
</script>

<Stack gap="lg">
	<header class="head">
		<Heading level={1}>Settings</Heading>
		<Text tone="faint">App-wide preferences, stored on this device.</Text>
	</header>

	<Card>
		<dl class="props">
			{#each SETTING_DEFS as def (def.key)}
				<div class="prop">
					<dt>
						<Text weight="semibold">{def.label}</Text>
						<Text size="sm" tone="faint">{def.description}</Text>
					</dt>
					<dd>
						<Switch
							checked={settings.state[def.key]}
							label={def.label}
							onclick={() => settings.toggle(def.key)}
						/>
					</dd>
				</div>
			{/each}
		</dl>
	</Card>
</Stack>

<style>
	.head {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
	}
	.props {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		margin: 0;
	}
	.prop {
		display: flex;
		align-items: flex-start;
		justify-content: space-between;
		gap: var(--sp-3);
	}
	/* Stack label over its help text; the switch stays pinned to the right. */
	.prop dt {
		display: flex;
		flex-direction: column;
		gap: 2px;
		min-width: 0;
	}
	.prop dd {
		margin: 0;
		flex: none;
	}
	.prop + .prop {
		border-top: 1px solid var(--border);
		padding-top: var(--sp-3);
	}
</style>
