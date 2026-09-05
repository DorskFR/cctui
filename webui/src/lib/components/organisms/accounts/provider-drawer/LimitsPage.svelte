<script lang="ts">
	import { CapBar, Input, Switch, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import type { UsageWindow } from '$lib/queries';
	import type { SoftEdit } from '../account-editor.logic';
	import { isUsdKey } from '$lib/components/molecules/usage-windows';
	import { capFromBar, capToBar, resetIn } from '$lib/components/molecules/cap-bar.logic';

	let {
		rows,
		windows = [],
		edits = $bindable({}),
		rate = $bindable({ rpm: null, tpm: null }),
		canPin = false,
		pinned = false,
		onpin
	}: {
		rows: { key: string; label: string }[];
		windows?: UsageWindow[];
		edits?: Record<string, SoftEdit>;
		rate?: { rpm: number | null; tpm: number | null };
		canPin?: boolean;
		pinned?: boolean;
		onpin?: () => void;
	} = $props();

	const now = Date.now();
	const live = $derived(new Map(windows.map((w) => [w.key, w])));
	const usedPct = (key: string) => {
		const u = live.get(key)?.utilization;
		return u === null || u === undefined ? null : Math.max(0, Math.min(100, Math.round(u)));
	};
</script>

<div class="page">
	<Text as="p" tone="muted" size="sm">{m.limits_intro()}</Text>

	{#each rows as row (row.key)}
		{#if edits[row.key]}
			{@const usd = isUsdKey(row.key)}
			{@const pct = usedPct(row.key)}
			{@const resets = resetIn(live.get(row.key)?.resets_at ?? null, now)}
			<div class="window">
				<div class="line">
					<Text as="span" size="sm" weight="medium">{row.label}</Text>
					<span class="spacer"></span>
					{#if usd}
						<label class="field">
							<Text as="span" size="xs" tone="muted">{m.limits_cap_usd()}</Text>
							<Input
								type="number"
								step="0.01"
								size="sm"
								mono
								width="72px"
								bind:value={edits[row.key].capUsd}
								placeholder={m.limits_never()}
							/>
						</label>
					{:else}
						<Text as="span" size="xs" tone="muted">
							{m.limits_cap({ pct: capToBar(edits[row.key].cap) })}
						</Text>
					{/if}
					<label class="field">
						<Text as="span" size="xs" tone="muted">{m.limits_bypass()}</Text>
						<Input
							type="number"
							min="0"
							step="1"
							size="sm"
							mono
							width="56px"
							bind:value={edits[row.key].bypass}
							placeholder={m.limits_never()}
						/>
						<Text as="span" size="xs" tone="muted">{m.limits_minutes()}</Text>
					</label>
				</div>
				{#if !usd}
					<CapBar
						size="lg"
						value={pct ?? 0}
						step={5}
						warnAt={75}
						readout={resets ?? ''}
						readoutWidth={resets ? '76px' : '0'}
						tooltip={m.capbar_tooltip({ pct: capToBar(edits[row.key].cap) })}
						bind:cap={
							() => capToBar(edits[row.key].cap), (v) => (edits[row.key].cap = capFromBar(v))
						}
					>
						{#snippet caption()}
							{#if pct !== null}
								<Text as="span" size="xs" tone="faint">{m.limits_using({ pct })}</Text>
							{:else}
								<Text as="span" size="xs" tone="faint">{m.softlimit_not_reported()}</Text>
							{/if}
						{/snippet}
					</CapBar>
				{/if}
			</div>
		{/if}
	{/each}

	<div class="rates">
		<label class="rate">
			<Text as="span" size="xs" tone="muted">{m.accounts_rate_rpm_label()}</Text>
			<Input
				type="number"
				min="0"
				step="1"
				size="sm"
				mono
				bind:value={rate.rpm}
				placeholder={m.limits_unlimited()}
			/>
		</label>
		<label class="rate">
			<Text as="span" size="xs" tone="muted">{m.accounts_rate_tpm_label()}</Text>
			<Input
				type="number"
				min="0"
				step="1"
				size="sm"
				mono
				bind:value={rate.tpm}
				placeholder={m.limits_unlimited()}
			/>
		</label>
	</div>
	<Text as="p" tone="faint" size="xs">{m.accounts_rate_limits_help()}</Text>

	{#if canPin}
		<div class="pin">
			<span class="pin-copy">
				<Text as="span" size="sm">{m.providers_header_pin()}</Text>
				<Text as="span" size="xs" tone="faint">{m.providers_header_pin_help()}</Text>
			</span>
			<Switch checked={pinned} label={m.providers_header_pin()} onclick={onpin} />
		</div>
	{/if}
</div>

<style>
	.page {
		display: flex;
		flex-direction: column;
		gap: var(--sp-4);
	}
	.window {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		padding-top: var(--sp-3);
		border-top: 1px solid var(--border);
	}
	.line {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		flex-wrap: wrap;
	}
	.spacer {
		flex: 1;
	}
	.field {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-1);
	}
	.rates {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: var(--sp-2);
		padding-top: var(--sp-3);
		border-top: 1px solid var(--border);
	}
	.rate {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
		min-width: 0;
	}
	.pin {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--sp-3);
		padding-top: var(--sp-3);
		border-top: 1px solid var(--border);
	}
	.pin-copy {
		display: flex;
		flex-direction: column;
		min-width: 0;
	}
</style>
