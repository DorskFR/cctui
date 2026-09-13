<script lang="ts">
	import { Button, ConfirmModal, Progress, Text } from '@dorsk/tsumikit';
	import SettingGroup from '$lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '$lib/components/molecules/SettingRow.svelte';
	import SettingSection from '$lib/components/molecules/SettingSection.svelte';
	import GuideRow from './GuideRow.svelte';
	import { buildCurriculum, guideDoneMap, guideEntries, replayGuide, resetGuides } from '$lib/guides';
	import type { GuideSectionId } from '$lib/guides';
	import { settings } from '$lib/settings.svelte';
	import { toasts } from '$lib/toast.svelte';
	import { m } from '$lib/paraglide/messages';

	/** The Replay button reports "starting", not the whole tour: `startGuide`
	 *  only settles when the run ends, and a refusal always beats this. */
	const START_SETTLE_MS = 600;

	let busy = $state<string | null>(null);
	let confirming = $state(false);
	let doneMap = $state<Record<string, boolean>>({});
	let runtimeTick = $state(0);

	$effect(() => {
		const bump = () => runtimeTick++;
		const observer = new MutationObserver(bump);
		observer.observe(document.documentElement, { attributes: true, attributeFilter: ['lang'] });
		let poll: ReturnType<typeof setInterval> | undefined;
		if (typeof window.__journey?.translate !== 'function') {
			poll = setInterval(() => {
				if (typeof window.__journey?.translate !== 'function') return;
				clearInterval(poll);
				bump();
			}, 200);
		}
		return () => {
			observer.disconnect();
			clearInterval(poll);
		};
	});

	const entries = $derived.by(() => {
		void runtimeTick;
		return guideEntries();
	});

	$effect(() => {
		const ids = entries.map((e) => e.id);
		void settings.onboarding;
		let alive = true;
		void guideDoneMap(ids).then((map) => {
			if (alive) doneMap = map;
		});
		return () => {
			alive = false;
		};
	});

	const curriculum = $derived(buildCurriculum(entries, settings.onboarding, doneMap));

	const SECTION_TITLE: Record<GuideSectionId, () => string> = {
		basics: () => m.settings_guides_section_basics(),
		setup: () => m.settings_guides_section_setup(),
		run: () => m.settings_guides_section_run(),
		master: () => m.settings_guides_section_master()
	};

	async function launch(id: string) {
		busy = id;
		const run = replayGuide(id);
		void run.then(
			(outcome) => {
				if (outcome.ok) return;
				if (outcome.reason === 'gated') {
					toasts.info(m.settings_guides_gated({ prerequisite: outcome.prerequisite }));
				} else {
					toasts.error(m.settings_guides_unavailable());
				}
			},
			() => toasts.error(m.settings_guides_unavailable())
		);
		await Promise.race([
			run.catch(() => undefined),
			new Promise((resolve) => setTimeout(resolve, START_SETTLE_MS))
		]);
		busy = null;
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
		<SettingRow label={m.settings_guides_progress_label()} selfLabelled wide>
			<span class="overall">
				<Progress
					block
					value={curriculum.earnedXp}
					max={curriculum.totalXp || 1}
					label={m.settings_guides_progress_label()}
				/>
				<span class="totals">
					<Text size="sm" weight="semibold" as="span">
						{m.settings_guides_xp_earned({
							earned: curriculum.earnedXp,
							total: curriculum.totalXp
						})}
					</Text>
					<Text size="sm" tone="faint" as="span">
						{m.settings_guides_progress_count({
							done: curriculum.doneCount,
							total: curriculum.totalCount
						})}
					</Text>
				</span>
			</span>
		</SettingRow>
	</SettingGroup>

	{#each curriculum.sections as section (section.id)}
		<SettingGroup title={SECTION_TITLE[section.id]()}>
			{#if section.locked}
				<SettingRow
					label={m.settings_guides_locked()}
					help={m.settings_guides_section_locked_by({ guides: section.lockedBy.join(', ') })}
					selfLabelled
					wide
					disabled
				/>
			{/if}
			{#each section.guides as guide (guide.id)}
				<GuideRow {guide} busy={busy === guide.id} onlaunch={launch} />
			{/each}
		</SettingGroup>
	{:else}
		<SettingGroup>
			<SettingRow label={m.settings_guides_empty()} selfLabelled wide />
		</SettingGroup>
	{/each}

	<SettingGroup>
		<SettingRow
			label={m.settings_guides_reset_label()}
			help={m.settings_guides_reset_help()}
			selfLabelled
		>
			<Button size="sm" variant="danger" onclick={() => (confirming = true)}>
				{m.settings_guides_reset()}
			</Button>
		</SettingRow>
	</SettingGroup>
</SettingSection>

<ConfirmModal
	bind:open={confirming}
	tone="danger"
	title={m.settings_guides_reset_confirm_title()}
	message={m.settings_guides_reset_confirm_message()}
	confirmLabel={m.settings_guides_reset_confirm()}
	cancelLabel={m.settings_guides_cancel()}
	onconfirm={reset}
/>

<style>
	.overall {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		width: 100%;
	}
	.totals {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		gap: var(--sp-2);
	}
</style>
