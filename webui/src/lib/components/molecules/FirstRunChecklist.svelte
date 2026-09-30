<script lang="ts">
	// The three things an instance needs before it can run anything, on the empty
	// states a new user actually lands on. Ticks come from the same probe registry
	// the guides read, so the checklist and a guide can never disagree about
	// whether a step is done — and nothing about the ticks is stored.
	import { useQueryClient } from '@tanstack/svelte-query';
	import { Button, Stack, Text } from '@dorsk/tsumikit';
	import { buildCurriculum, guideEntries, guideOptions, replayGuide } from '$lib/guides';
	import { GUIDES_ROUTE, startFailureMessage } from '$lib/journey';
	import { createProbes } from '$lib/journeys/probes';
	import { settings } from '$lib/settings.svelte';
	import { toasts } from '$lib/toast.svelte';
	import { m } from '$lib/paraglide/messages';

	const ROWS = [
		{ guide: 'accounts-pools', probe: 'accounts', label: () => m.onboarding_step_account() },
		{ guide: 'enroll-machine', probe: 'machines.enrolled', label: () => m.onboarding_step_machine() },
		{ guide: 'spawn-session', probe: 'sessions', label: () => m.onboarding_step_session() }
	] as const;

	const probes = createProbes(useQueryClient());

	let ticks = $state<Record<string, boolean>>({});
	let busy = $state<string | null>(null);
	let loaded = $state(false);

	$effect(() => {
		void settings.onboarding;
		let alive = true;
		void Promise.all(
			ROWS.map(async (r) => {
				try {
					return [r.probe, Boolean(await probes[r.probe]())] as const;
				} catch {
					return [r.probe, false] as const;
				}
			})
		).then((pairs) => {
			if (!alive) return;
			ticks = Object.fromEntries(pairs);
			loaded = true;
		});
		return () => {
			alive = false;
		};
	});

	const done = $derived(ROWS.every((r) => ticks[r.probe]));
	const show = $derived(loaded && !done && settings.onboarding.checklistDismissed !== true);

	async function launch(id: string) {
		busy = id;
		try {
			const view = buildCurriculum(guideEntries(), settings.onboarding)
				.sections.flatMap((s) => s.guides)
				.find((g) => g.id === id);
			if (!view) return;
			const say = startFailureMessage(await replayGuide(id, guideOptions(view)));
			if (say) toasts.info(say);
		} catch {
			toasts.error(m.journey_unavailable());
		} finally {
			busy = null;
		}
	}
</script>

{#if show}
	<div class="checklist" data-journey="first-run">
		<Stack gap="sm">
			<Text size="sm" weight="semibold">{m.onboarding_checklist_title()}</Text>
			{#each ROWS as row, i (row.guide)}
				<div class="row" class:ticked={ticks[row.probe]}>
					<span class="mark" aria-hidden="true">{ticks[row.probe] ? '◆' : i + 1}</span>
					<Text size="sm" tone={ticks[row.probe] ? 'faint' : 'default'}>{row.label()}</Text>
					{#if !ticks[row.probe]}
						<span class="act">
							<Button
								size="sm"
								variant="ghost"
								loading={busy === row.guide}
								onclick={() => launch(row.guide)}
								data-journey="first-run-start"
								data-journey-key={row.guide}
							>
								{m.onboarding_checklist_show()}
							</Button>
						</span>
					{/if}
				</div>
			{/each}
			<div class="foot">
				<a href={GUIDES_ROUTE}><Text size="xs" tone="faint">{m.onboarding_checklist_all()}</Text></a>
				<Button
					size="sm"
					variant="ghost"
					onclick={() => settings.setOnboarding({ checklistDismissed: true })}
				>
					{m.onboarding_checklist_dismiss()}
				</Button>
			</div>
		</Stack>
	</div>
{/if}

<style>
	.checklist {
		padding: var(--sp-4);
		border: 1px solid var(--border);
		border-radius: var(--radius-md, 8px);
		background: var(--surface);
		max-width: 32rem;
		margin-inline: auto;
		text-align: start;
	}
	.row {
		display: flex;
		align-items: center;
		gap: var(--sp-3);
	}
	.row.ticked .mark {
		color: var(--ok);
	}
	.mark {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 1.5rem;
		height: 1.5rem;
		flex: none;
		border-radius: 50%;
		border: 1px solid var(--border);
		font-size: var(--fs-xs);
		color: var(--text-faint);
	}
	.act {
		display: inline-flex;
		margin-inline-start: auto;
	}
	.foot {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--sp-2);
		padding-block-start: var(--sp-2);
		border-top: 1px solid var(--border);
	}
</style>
