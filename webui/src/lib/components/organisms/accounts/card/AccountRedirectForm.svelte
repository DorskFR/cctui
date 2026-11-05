<script lang="ts">
	import { Button, Checkbox, Select, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import { openFamilies, type RedirectChip, type RedirectTarget } from './redirect.logic';

	let {
		targets,
		ownFamilies,
		redirects,
		onsubmit,
		oncancel
	}: {
		/** Already narrowed to targets with an open family. */
		targets: RedirectTarget[];
		ownFamilies: string[];
		redirects: RedirectChip[];
		onsubmit: (targetId: string, untilHours: number | null, families: string[]) => void;
		oncancel: () => void;
	} = $props();

	let target = $state('');
	let hours = $state('');
	const targetFamilies = $derived.by(() => {
		const t = targets.find((t) => t.id === target);
		return t ? openFamilies(t, ownFamilies, redirects) : [];
	});
	let families = $derived<string[]>(targetFamilies);

	function toggleFamily(f: string) {
		families = families.includes(f) ? families.filter((x) => x !== f) : [...families, f];
	}

	function submit() {
		if (!target || families.length === 0) return;
		onsubmit(target, hours === '' ? null : Number(hours), families);
	}
</script>

<div class="redirect-form">
	<Text as="span" tone="muted" size="sm">{m.accounts_redirect_pick()}</Text>
	<Select bind:value={target} aria-label={m.accounts_redirect_pick()}>
		<option value="" disabled></option>
		{#each targets as t (t.id)}
			<option value={t.id}>{t.name}</option>
		{/each}
	</Select>
	{#each targetFamilies as f (f)}
		<Checkbox label={f} checked={families.includes(f)} onchange={() => toggleFamily(f)} />
	{/each}
	<Select bind:value={hours} aria-label={m.a11y_redirect_expiry()}>
		<option value="">{m.accounts_redirect_no_expiry()}</option>
		{#each ['1', '5', '24'] as h (h)}
			<option value={h}>{m.accounts_redirect_hours({ n: h })}</option>
		{/each}
	</Select>
	<Button size="sm" variant="primary" disabled={!target || families.length === 0} onclick={submit}>
		{m.accounts_redirect_set()}
	</Button>
	<Button size="sm" onclick={oncancel}>{m.common_cancel()}</Button>
</div>

<style>
	.redirect-form {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--sp-2);
		padding: var(--sp-2) var(--sp-4);
		background: var(--bg-elevated-2);
	}
</style>
