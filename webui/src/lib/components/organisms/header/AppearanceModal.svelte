<script lang="ts">
	import { FontScalePicker, Modal, SCALE_LEVELS, Text, ThemePicker, fontScale, theme } from '@dorsk/tsumikit';
	import { themePickerTitle } from '$lib/components/molecules/themePicker.logic';
	import { scaleLabel } from './appearance.logic';
	import { m } from '$lib/paraglide/messages';

	let { onclose }: { onclose: () => void } = $props();

	const themeText = $derived(
		themePickerTitle(theme.all, theme.pref, theme.resolved, m.theme_auto_label())
	);
	const sizeText = $derived(scaleLabel(SCALE_LEVELS, fontScale.levelId));
</script>

<Modal title={m.shell_appearance_title()} size="sm" {onclose}>
	{#snippet body()}
		<div class="rows">
			<div class="row">
				<span class="what">
					<Text size="sm" weight="semibold">{m.shell_appearance_theme()}</Text>
					<Text size="sm" tone="muted">{themeText}</Text>
				</span>
				<ThemePicker
					auto
					autoLabel={m.theme_auto_label()}
					autoHelp={m.theme_auto_help()}
					lightLabel={m.theme_group_light()}
					darkLabel={m.theme_group_dark()}
					variant="default"
					box="md"
					placement="bottom-end"
				/>
			</div>
			<div class="row">
				<span class="what">
					<Text size="sm" weight="semibold">{m.shell_appearance_text_size()}</Text>
					<Text size="sm" tone="muted">{sizeText}</Text>
				</span>
				<FontScalePicker variant="default" box="md" placement="bottom-end" />
			</div>
			<a class="more" href="/settings/interface#theme" onclick={onclose}>{m.shell_appearance_more()}</a>
		</div>
	{/snippet}
</Modal>

<style>
	.rows {
		display: flex;
		flex-direction: column;
	}
	.row {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--sp-4);
		padding: var(--sp-3) 0;
		border-bottom: 1px solid var(--divider, var(--border));
	}
	.row:first-child {
		padding-top: 0;
	}
	.what {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
		min-width: 0;
	}
	.more {
		align-self: flex-start;
		margin-top: var(--sp-3);
		color: var(--accent);
		font-size: var(--fs-sm);
		text-decoration: none;
	}
	.more:hover {
		text-decoration: underline;
	}
</style>
