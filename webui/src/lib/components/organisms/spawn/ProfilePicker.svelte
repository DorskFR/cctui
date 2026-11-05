<script lang="ts">
	// The selected profile as one card that drops a listbox of every profile.
	// Built on Popover so the card and the sheet are this component's own
	// markup: the trigger is bare and fills the card, and the panel is sized to
	// the card and placed flush against it.
	import type { SessionProfile } from '@bindings/SessionProfile';
	import { Button, Icon, IconButton, Popover } from '@dorsk/tsumikit';
	import ProfileRow from './ProfileRow.svelte';
	import type { BadgeLook } from './accountBadge';
	import { pickerKeyAction, typeaheadIndex } from './profilePickerKeys';
	import type { ProfileSpecForm } from './profiles';
	import { m } from '$lib/paraglide/messages';

	let {
		profiles,
		selectedId,
		open = $bindable(false),
		attached = false,
		editing = false,
		busy = false,
		specFor,
		chainOf,
		badgeOf,
		onselect,
		onedit,
		onnew,
		manageHref
	}: {
		/** Already in display order. */
		profiles: SessionProfile[];
		selectedId: string | null;
		open?: boolean;
		/** A form hangs under the card. */
		attached?: boolean;
		editing?: boolean;
		busy?: boolean;
		specFor: (p: SessionProfile) => ProfileSpecForm;
		chainOf: (spec: ProfileSpecForm) => string;
		badgeOf: (spec: ProfileSpecForm) => BadgeLook;
		onselect: (id: string) => void;
		/** Absent: no cog. */
		onedit?: () => void;
		onnew?: () => void;
		/** The footer links here instead of offering "New profile". */
		manageHref?: string;
	} = $props();

	const uid = Math.random().toString(36).slice(2, 8);
	const listId = `pp-list-${uid}`;
	const optionId = (i: number) => `pp-opt-${uid}-${i}`;
	const TRIGGER_STYLE =
		'display:flex;width:100%;min-width:0;padding:0;border:none;background:none;color:inherit;font:inherit;text-align:start;cursor:pointer';

	let cardEl = $state<HTMLElement | null>(null);
	let listEl = $state<HTMLElement | null>(null);
	let cardWidth = $state(0);
	let side = $state<'top' | 'bottom'>('bottom');
	let active = $state(-1);
	let typed = '';
	let typedAt = 0;

	const selectedIndex = $derived(profiles.findIndex((p) => p.id === selectedId));
	const current = $derived(selectedIndex >= 0 ? profiles[selectedIndex] : undefined);
	const panelStyle = $derived(
		'min-width:0;max-width:none;padding:0;border:none;background:none;box-shadow:none' +
			(cardWidth > 0 ? `;width:${cardWidth}px` : '')
	);

	const triggerEl = () => cardEl?.querySelector<HTMLElement>('[data-journey="profile-trigger"]') ?? null;
	const panelEl = () => cardEl?.querySelector<HTMLElement>('[popover]') ?? null;

	$effect(() => {
		const el = cardEl;
		if (!el || typeof ResizeObserver === 'undefined') return;
		const ro = new ResizeObserver(() => (cardWidth = el.offsetWidth));
		ro.observe(el);
		return () => ro.disconnect();
	});

	$effect(() => {
		triggerEl()?.setAttribute('role', 'combobox');
	});

	// Popover flips the panel above the card when it does not fit below and
	// records the side it picked; the sheet attaches on that side.
	$effect(() => {
		const panel = panelEl();
		if (!panel) return;
		const read = () => (side = panel.dataset.side === 'top' ? 'top' : 'bottom');
		const mo = new MutationObserver(read);
		mo.observe(panel, { attributes: true, attributeFilter: ['data-side'] });
		read();
		return () => mo.disconnect();
	});

	function revealActive() {
		const row = listEl?.children[active] as HTMLElement | undefined;
		if (!listEl || !row) return;
		const top = row.offsetTop;
		const bottom = top + row.offsetHeight;
		if (top < listEl.scrollTop) listEl.scrollTop = top;
		else if (bottom > listEl.scrollTop + listEl.clientHeight) listEl.scrollTop = bottom - listEl.clientHeight;
	}

	function onopen() {
		active = selectedIndex >= 0 ? selectedIndex : 0;
		typed = '';
		revealActive();
	}

	function close() {
		open = false;
		triggerEl()?.focus();
	}

	function choose(i: number) {
		const p = profiles[i];
		if (!p) return;
		if (p.id !== selectedId) onselect(p.id);
		close();
	}

	function typeahead(char: string) {
		const now = Date.now();
		typed = now - typedAt > 700 ? char : typed + char;
		typedAt = now;
		const from = open ? active : selectedIndex;
		const hit = typeaheadIndex(
			profiles.map((p) => p.name),
			typed,
			from
		);
		if (hit === undefined) return;
		if (open) {
			active = hit;
			revealActive();
		} else onselect(profiles[hit].id);
	}

	function onkeydowncapture(e: KeyboardEvent) {
		if (e.target !== triggerEl()) return;
		const action = pickerKeyAction(e, {
			open,
			count: profiles.length,
			index: open ? active : selectedIndex
		});
		if (!action) return;
		if (action.type === 'close' && e.key === 'Tab') {
			open = false;
			return;
		}
		e.preventDefault();
		e.stopPropagation();
		switch (action.type) {
			case 'open':
				open = true;
				break;
			case 'close':
				close();
				break;
			case 'select':
				choose(active);
				break;
			case 'move':
				active = action.index;
				revealActive();
				break;
			case 'change':
				onselect(profiles[action.index].id);
				break;
			case 'type':
				typeahead(action.char);
				break;
		}
	}
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="card" class:no-tools={!onedit} bind:this={cardEl} {onkeydowncapture}>
	<Popover
		bind:open
		bare
		gap={0}
		label={m.spawn_profiles_aria()}
		placement="bottom-start"
		role="group"
		haspopup="listbox"
		style={TRIGGER_STYLE}
		{panelStyle}
		{onopen}
		data-journey="profile-trigger"
		aria-controls={listId}
		aria-activedescendant={open && active >= 0 ? optionId(active) : undefined}
	>
		{#snippet trigger()}
			<span
				class="face"
				class:flat-bottom={attached || (open && side === 'bottom')}
				class:flat-top={open && side === 'top'}
			>
				{#if current}
					<ProfileRow
						id={current.id}
						name={current.name}
						chain={chainOf(specFor(current))}
						badge={badgeOf(specFor(current))}
						variant="card"
					/>
				{:else}
					<span class="placeholder">{m.spawn_profiles_aria()}</span>
				{/if}
				<span class="chevron" class:open aria-hidden="true"><Icon name="chevron-down" /></span>
			</span>
		{/snippet}
		<div class="sheet" class:top={side === 'top'}>
			<div class="list" role="listbox" id={listId} aria-label={m.spawn_profiles_aria()} bind:this={listEl}>
				{#each profiles as p, i (p.id)}
					<!-- svelte-ignore a11y_click_events_have_key_events -->
					<div
						class="row"
						role="option"
						id={optionId(i)}
						tabindex="-1"
						aria-selected={i === selectedIndex}
						data-active={i === active ? '' : undefined}
						data-selected={i === selectedIndex ? '' : undefined}
						onclick={() => choose(i)}
						onmousedown={(e) => e.preventDefault()}
						onpointermove={() => (active = i)}
					>
						<ProfileRow id={p.id} name={p.name} chain={chainOf(specFor(p))} badge={badgeOf(specFor(p))} />
					</div>
				{/each}
			</div>
			<div class="footer">
				{#if manageHref}
					<Button data-journey="manage-profiles" variant="link" size="sm" href={manageHref}>
						{m.launch_manage_profiles()}
					</Button>
				{:else if onnew}
					<Button
						data-journey="new-profile"
						variant="link"
						size="sm"
						disabled={busy}
						onclick={() => {
							open = false;
							onnew?.();
						}}>{m.spawn_profile_new()}</Button
					>
				{/if}
			</div>
		</div>
	</Popover>
	{#if current && onedit}
		<span class="tools">
			<span class="cog">
				<IconButton
					icon="settings"
					label={m.spawn_profile_adjust()}
					inline
					size={14}
					pressed={editing}
					aria-expanded={editing}
					onclick={onedit}
				/>
			</span>
			<span class="divider" aria-hidden="true"></span>
		</span>
	{/if}
</div>

<style>
	.card {
		--card-pad: var(--sp-2);
		--tools-w: calc(24px + var(--sp-2) * 2 + 1px);
		position: relative;
		min-width: 0;
	}
	.face {
		flex: 1 1 auto;
		min-width: 0;
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--sp-2);
		box-sizing: border-box;
		padding: var(--card-pad);
		border: var(--edge-width) solid var(--accent);
		border-radius: var(--r-md);
		background: color-mix(in srgb, var(--accent) 14%, var(--bg));
		color: var(--accent);
		transition: background-color var(--dur-fast) var(--ease);
	}
	@media (hover: hover) and (pointer: fine) {
		.face:hover {
			background: color-mix(in srgb, var(--accent) 20%, var(--bg));
		}
	}
	.face.flat-bottom {
		border-end-start-radius: 0;
		border-end-end-radius: 0;
	}
	.face.flat-top {
		border-start-start-radius: 0;
		border-start-end-radius: 0;
	}
	.placeholder {
		color: var(--text-muted);
		font-size: var(--control-fs-md);
	}
	/* The cog overlays the card in the room the chevron's margin keeps free. */
	.chevron {
		display: inline-flex;
		flex: none;
		margin-inline-start: var(--tools-w);
		color: color-mix(in srgb, var(--accent) 70%, var(--text-muted));
		--icon-size: var(--control-icon-md);
		transition: rotate var(--dur-fast) var(--ease-out);
	}
	.no-tools .chevron {
		margin-inline-start: 0;
	}
	.chevron.open {
		rotate: 180deg;
	}
	.tools {
		position: absolute;
		inset-block: 0;
		inset-inline-end: calc(var(--card-pad) + var(--control-icon-md) + var(--sp-2));
		width: var(--tools-w);
		display: flex;
		align-items: center;
		justify-content: flex-end;
		gap: var(--sp-2);
		color: var(--accent);
		pointer-events: none;
	}
	.cog {
		display: inline-flex;
		pointer-events: auto;
	}
	.divider {
		align-self: stretch;
		width: 1px;
		margin-block: var(--sp-2);
		background: color-mix(in srgb, var(--accent) 35%, transparent);
	}
	.sheet {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
		box-sizing: border-box;
		max-height: min(70vh, 28rem);
		padding: var(--sp-1);
		border: var(--edge-width) solid var(--accent);
		border-top: none;
		border-radius: 0 0 var(--r-md) var(--r-md);
		background: var(--raised-fill);
		box-shadow: var(--raised-shadow);
	}
	.sheet.top {
		border-top: var(--edge-width) solid var(--accent);
		border-bottom: none;
		border-radius: var(--r-md) var(--r-md) 0 0;
	}
	.list {
		display: flex;
		flex-direction: column;
		gap: var(--sp-0-5);
		min-height: 0;
		overflow-y: auto;
		overscroll-behavior: contain;
	}
	.row {
		padding: var(--sp-1);
		border-radius: var(--r-sm);
		cursor: pointer;
		outline: none;
		transition: background-color var(--dur-fast) var(--ease);
	}
	.row[data-active] {
		background: var(--bg-elevated-2);
	}
	.row[data-selected] {
		background: color-mix(in srgb, var(--accent) 12%, transparent);
	}
	.row[data-selected][data-active] {
		background: color-mix(in srgb, var(--accent) 18%, var(--bg-elevated-2));
	}
	.footer {
		flex: none;
		display: flex;
		padding-top: var(--sp-1);
		border-top: var(--edge-width) solid var(--surface-edge);
	}
</style>
