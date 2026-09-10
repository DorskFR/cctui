<script lang="ts">
	import { Badge, Button } from '@dorsk/tsumikit';
	import SettingGroup from '$lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '$lib/components/molecules/SettingRow.svelte';
	import SettingSection from '$lib/components/molecules/SettingSection.svelte';
	import { guideEntries, guideStatus, replayGuide, resetGuides } from '$lib/guides';
	import type { GuideStatus } from '$lib/guides';
	import { settings } from '$lib/settings.svelte';
	import { toasts } from '$lib/toast.svelte';
	import { m } from '$lib/paraglide/messages';

	const entries = guideEntries();
	const statuses = $derived(
		new Map(entries.map((e) => [e.id, guideStatus(e, settings.onboarding)] as const))
	);
	let busy = $state<string | null>(null);

	const STATUS_TONE = { done: 'ok', 'in-progress': 'warn', 'not-started': 'muted' } as const;

	function statusLabel(s: GuideStatus): string {
		if (s === 'done') return m.settings_guides_status_done();
		if (s === 'in-progress') return m.settings_guides_status_in_progress();
		return m.settings_guides_status_not_started();
	}

	async function replay(id: string) {
		busy = id;
		try {
			await replayGuide(id);
		} finally {
			busy = null;
		}
	}

	function reset() {
		resetGuides();
		toasts.ok(m.settings_guides_reset_done());
	}
</script>

<SettingSection
	id="guides"
	icon="◇"
	title={m.settings_guides_title()}
	description={m.settings_guides_description()}
>
	<SettingGroup>
		{#each entries as e (e.id)}
			{@const status = statuses.get(e.id) ?? 'not-started'}
			<SettingRow label={e.title} help={e.description} selfLabelled>
				<span class="row">
					<Badge tone={STATUS_TONE[status]} size="sm" border>{statusLabel(status)}</Badge>
					<Button
						size="sm"
						loading={busy === e.id}
						onclick={() => replay(e.id)}
					>
						{status === 'in-progress' ? m.settings_guides_resume() : m.settings_guides_replay()}
					</Button>
				</span>
			</SettingRow>
		{:else}
			<SettingRow label={m.settings_guides_empty()} selfLabelled wide />
		{/each}
	</SettingGroup>

	<SettingGroup>
		<SettingRow
			label={m.settings_guides_reset_label()}
			help={m.settings_guides_reset_help()}
			selfLabelled
		>
			<Button size="sm" variant="danger" onclick={reset}>{m.settings_guides_reset()}</Button>
		</SettingRow>
	</SettingGroup>
</SettingSection>

<style>
	.row {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-2);
		justify-content: flex-end;
		width: 100%;
	}
</style>
