<script lang="ts">
	import { untrack } from 'svelte';
	import type { SessionProfile } from '@bindings/SessionProfile';
	import type { OAuthAccount } from '#lib/queries/index.ts';
	import ProfileList from './ProfileList.svelte';
	import type { ProfileSpecForm } from './profiles';

	let {
		profiles,
		accounts,
		oncreate,
		onsave,
		ondelete,
		manageHref,
		onmanage,
		onready
	}: {
		profiles: SessionProfile[];
		accounts: OAuthAccount[];
		oncreate: (name: string, spec: ProfileSpecForm) => Promise<boolean>;
		onsave: (id: string, name: string, spec: ProfileSpecForm) => void;
		ondelete: (id: string) => void;
		manageHref?: string;
		onmanage?: () => void;
		onready: (s: { selectedId: string | null; oneOff: ProfileSpecForm | null }) => void;
	} = $props();

	const held = $state<{ selectedId: string | null; oneOff: ProfileSpecForm | null }>({
		selectedId: untrack(() => profiles[0]?.id ?? null),
		oneOff: null
	});
	untrack(() => onready(held));
</script>

<ProfileList
	{profiles}
	bind:selectedId={held.selectedId}
	bind:oneOff={held.oneOff}
	{accounts}
	usage={[]}
	machineId=""
	{oncreate}
	{onsave}
	{ondelete}
	{manageHref}
	{onmanage}
/>
