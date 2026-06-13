<script lang="ts">
	// Working-directory picker, extracted from SpawnModal: a recent-dirs select
	// plus a path input with live directory autocomplete. The typed value is split
	// at the last `/` — `parent` is listed on the machine and filtered by `prefix`
	// locally, so a request only fires when the user crosses a `/` boundary.
	// Dotdirs surface only once the typed segment starts with a dot.
	import { useMachineDirs } from '$lib/queries';
	import { Field, Input } from '@dorsk/tsumikit';
	import Select from '$lib/components/atoms/Select.svelte';

	let {
		machineId,
		value = $bindable(),
		recentDirs
	}: {
		machineId: string;
		value: string;
		recentDirs: string[];
	} = $props();

	let cwdFocused = $state(false);
	let cwdHighlight = $state(-1);
	const cwdParent = $derived.by(() => {
		const i = value.lastIndexOf('/');
		if (i < 0) return '';
		return i === 0 ? '/' : value.slice(0, i);
	});
	const cwdPrefix = $derived.by(() => {
		const i = value.lastIndexOf('/');
		return i < 0 ? value : value.slice(i + 1);
	});
	const machineDirs = useMachineDirs(
		() => (cwdFocused ? machineId : ''),
		() => cwdParent
	);
	const cwdSuggestions = $derived.by(() => {
		const dirs = $machineDirs.data?.dirs ?? [];
		const prefix = cwdPrefix.toLowerCase();
		const showHidden = prefix.startsWith('.');
		return dirs
			.filter((d) => (showHidden || !d.startsWith('.')) && d.toLowerCase().startsWith(prefix))
			.slice(0, 50);
	});
	$effect(() => {
		void cwdSuggestions; // reset highlight whenever the list changes
		cwdHighlight = -1;
	});
	function pickCwd(name: string) {
		value = `${cwdParent === '/' ? '' : cwdParent}/${name}/`;
		cwdHighlight = -1;
	}
	function cwdKeydown(e: KeyboardEvent) {
		if (!cwdFocused || !cwdSuggestions.length) return;
		if (e.key === 'ArrowDown') {
			e.preventDefault();
			cwdHighlight = (cwdHighlight + 1) % cwdSuggestions.length;
		} else if (e.key === 'ArrowUp') {
			e.preventDefault();
			cwdHighlight = (cwdHighlight - 1 + cwdSuggestions.length) % cwdSuggestions.length;
		} else if ((e.key === 'Enter' || e.key === 'Tab') && cwdHighlight >= 0) {
			e.preventDefault();
			pickCwd(cwdSuggestions[cwdHighlight]);
		} else if (e.key === 'Tab' && cwdSuggestions.length === 1) {
			e.preventDefault();
			pickCwd(cwdSuggestions[0]);
		} else if (e.key === 'Escape') {
			e.stopPropagation(); // keep the modal open; just dismiss the list
			cwdFocused = false;
		}
	}
</script>

<Field label="Working directory" for="sp-cwd">
	{#if recentDirs.length}
		<Select
			class="mono"
			aria-label="Recent directories"
			value={recentDirs.includes(value) ? value : ''}
			onchange={(e: Event) => (value = (e.currentTarget as HTMLSelectElement).value)}
		>
			<option value="">Recent directories…</option>
			{#each recentDirs as d (d)}<option value={d}>{d}</option>{/each}
		</Select>
	{/if}
	<div class="cwd-combo">
		<Input
			id="sp-cwd"
			mono
			placeholder="/home/user/project"
			autocomplete="off"
			bind:value
			onfocus={() => (cwdFocused = true)}
			oninput={() => (cwdFocused = true)}
			onblur={() => setTimeout(() => (cwdFocused = false), 150)}
			onkeydown={cwdKeydown}
		/>
		{#if cwdFocused && cwdSuggestions.length}
			<ul class="cwd-suggestions" role="listbox" aria-label="Directory suggestions">
				{#each cwdSuggestions as d, i (d)}
					<!-- svelte-ignore a11y_click_events_have_key_events -->
					<li
						role="option"
						aria-selected={i === cwdHighlight}
						class:active={i === cwdHighlight}
						onmousedown={(e) => {
							e.preventDefault();
							pickCwd(d);
						}}
					>
						{d}/
					</li>
				{/each}
			</ul>
		{/if}
	</div>
</Field>

<style>
	.cwd-combo {
		position: relative;
	}
	.cwd-suggestions {
		position: absolute;
		top: 100%;
		left: 0;
		right: 0;
		z-index: 10;
		margin: 2px 0 0;
		padding: var(--sp-1);
		list-style: none;
		max-height: 220px;
		overflow-y: auto;
		background: var(--bg);
		border: 1px solid var(--border-strong);
		border-radius: var(--r-md);
		font-size: var(--fs-xs);
	}
	.cwd-suggestions li {
		padding: 2px var(--sp-2);
		border-radius: var(--r-sm);
		cursor: pointer;
	}
	.cwd-suggestions li:hover,
	.cwd-suggestions li.active {
		background: color-mix(in srgb, var(--c-blue) 14%, var(--bg));
		color: var(--c-blue);
	}
</style>
