<script lang="ts">
	import { FontScalePicker, Modal, Text, ThemePicker } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	let { onclose }: { onclose: () => void } = $props();
</script>

<Modal title={m.shell_appearance_title()} size="sm" {onclose}>
	{#snippet body()}
		<div class="rows">
			<div class="row">
				<Text size="sm">{m.shell_appearance_theme()}</Text>
				<ThemePicker
					auto
					autoLabel={m.theme_auto_label()}
					autoHelp={m.theme_auto_help()}
					lightLabel={m.theme_group_light()}
					darkLabel={m.theme_group_dark()}
					box="md"
					placement="bottom-end"
				/>
			</div>
			<div class="row">
				<Text size="sm">{m.shell_appearance_text_size()}</Text>
				<FontScalePicker placement="bottom-end" />
			</div>
			<a class="more" href="/settings/interface#theme" onclick={onclose}>{m.shell_appearance_more()}</a>
		</div>
	{/snippet}
</Modal>

<style>
	.rows {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
	}
	.row {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--sp-4);
		min-height: var(--hit-min, 24px);
	}
	.more {
		align-self: flex-start;
		color: var(--accent);
		font-size: var(--fs-sm);
		text-decoration: none;
	}
	.more:hover {
		text-decoration: underline;
	}
</style>
