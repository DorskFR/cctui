<script lang="ts">
	import type { SessionProfile } from '@bindings/SessionProfile';
	import type { AccountUsageEntry, OAuthAccount } from '$lib/queries';
	import ProfileRow from './ProfileRow.svelte';
	import ProfileAdjust from './ProfileAdjust.svelte';
	import { claudeModels } from './options';
	import { specChain, specOf, type ProfileSpec } from './profiles';
	import { profileUsage } from '$lib/spawnMemory';
	import { m } from '$lib/paraglide/messages';

	let {
		profiles,
		selectedId = $bindable(),
		oneOff = $bindable(),
		accounts,
		usage,
		usageRaw,
		machineId,
		busy = false,
		oncreate,
		onsave,
		ondelete
	}: {
		profiles: SessionProfile[];
		selectedId: string | null;
		oneOff: ProfileSpec | null;
		accounts: OAuthAccount[];
		usage: AccountUsageEntry[];
		usageRaw: string;
		machineId: string;
		busy?: boolean;
		oncreate: () => void;
		onsave: (id: string, name: string, spec: ProfileSpec) => void;
		ondelete: (id: string) => void;
	} = $props();

	let openId = $state<string | null>(null);

	const chainLabels = $derived({
		auto: m.spawn_account_auto(),
		defaultModel: m.spawn_model_default(),
		defaultEffort: m.spawn_effort_default(),
		defaultMode: m.spawn_mode_default_label()
	});
	const modelLabel = (harness: string, alias: string) =>
		harness === 'codex' ? alias : (claudeModels.find((x) => x.v === alias)?.label ?? alias);

	function usageText(id: string): string {
		const u = profileUsage(usageRaw, id);
		if (!u) return '';
		if ('week' in u) return m.spawn_profile_uses_week({ count: u.week });
		const days = Math.floor((Date.now() - u.lastAt) / 86_400_000);
		return days < 1 ? m.spawn_profile_last_used_today() : m.spawn_profile_last_used_days({ days });
	}

	function select(id: string) {
		if (id !== selectedId) oneOff = null;
		selectedId = id;
	}
	function toggle(id: string) {
		openId = openId === id ? null : id;
	}
	function useOnce(p: SessionProfile, spec: ProfileSpec) {
		selectedId = p.id;
		oneOff = spec;
		openId = null;
	}
	function save(p: SessionProfile, name: string, spec: ProfileSpec) {
		if (selectedId === p.id) oneOff = null;
		openId = null;
		onsave(p.id, name, spec);
	}
</script>

<div class="list" role="radiogroup" aria-label={m.spawn_profiles_aria()}>
	{#each profiles as p (p.id)}
		{@const spec = selectedId === p.id && oneOff ? oneOff : specOf(p)}
		<ProfileRow
			id={p.id}
			name={p.name}
			chain={specChain(spec, accounts, chainLabels, modelLabel)}
			usage={usageText(p.id)}
			selected={selectedId === p.id}
			open={openId === p.id}
			onselect={() => select(p.id)}
			ontoggle={() => toggle(p.id)}
		>
			<ProfileAdjust
				profile={p}
				initial={spec}
				{accounts}
				{usage}
				{machineId}
				{busy}
				onuseonce={(s) => useOnce(p, s)}
				onsave={(name, s) => save(p, name, s)}
				ondelete={() => {
					openId = null;
					ondelete(p.id);
				}}
			/>
		</ProfileRow>
	{/each}
	<div class="new">
		<button type="button" class="new-link" disabled={busy} onclick={oncreate}>
			{m.spawn_profile_new()}
		</button>
	</div>
</div>

<style>
	.list {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
	.new {
		display: flex;
		justify-content: flex-end;
		padding: 0 var(--sp-1);
	}
	.new-link {
		background: none;
		border: none;
		padding: 0;
		font: inherit;
		font-size: var(--fs-xs);
		color: var(--link);
		cursor: pointer;
	}
	.new-link:hover {
		text-decoration: underline;
	}
	.new-link:disabled {
		opacity: 0.5;
		cursor: default;
	}
</style>
