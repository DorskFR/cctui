<script lang="ts">
	import { CopyButton } from '@dorsk/tsumikit';
	import { statusDotClass, type DiagnoseBlockSummary } from '$lib/diagnoseRows';
	import { m } from '$lib/paraglide/messages';

	let { blocks }: { blocks: DiagnoseBlockSummary[] } = $props();
</script>

<div class="blocks">
	{#each blocks as b (b.block)}
		<details class="block" data-block={b.block} data-status={b.status} open={b.status !== 'ok'}>
			<summary class="line">
				<span class="dot {statusDotClass(b.status)}"></span>
				<span class="title">{b.title}</span>
				<span class="short">{b.short}</span>
			</summary>
			{#each b.rows as r (r.label)}
				{#if r.status === 'ok'}
					<div class="row" data-status="ok">
						<span class="dot {statusDotClass(r.status)}"></span>
						<span class="label">{r.label}</span>
						<span class="short" title={r.detail}>{r.short}</span>
					</div>
				{:else}
					<div class="row open" data-status={r.status}>
						<span class="dot {statusDotClass(r.status)}"></span>
						<span class="label">{r.label}</span>
						<span class="short">{r.short}</span>
						{#if r.detail}
							<CopyButton text={r.detail} variant="ghost" box="xs" label={m.diagnose_copy_detail()} />
							<pre class="detail">{r.detail}</pre>
						{/if}
					</div>
				{/if}
			{/each}
		</details>
	{/each}
</div>

<style>
	.blocks {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
	}
	.block {
		border: 1px solid var(--border);
		border-radius: var(--r-md);
		padding: var(--sp-1) var(--sp-2);
	}
	.block[data-status='error'] {
		border-color: var(--danger);
	}
	.block[data-status='warn'] {
		border-color: var(--warn);
	}
	.line {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		cursor: pointer;
	}
	.title {
		font-weight: 600;
		white-space: nowrap;
	}
	.short {
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		color: var(--text-muted);
	}
	.row {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--sp-2);
		padding: 2px 0 2px var(--sp-3);
	}
	.label {
		white-space: nowrap;
	}
	.detail {
		flex-basis: 100%;
		margin: 0;
		white-space: pre-wrap;
		word-break: break-word;
		font-family: var(--font-mono, monospace);
		font-size: var(--fs-xs);
		max-height: 12rem;
		overflow: auto;
	}
</style>
