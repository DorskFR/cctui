<script lang="ts">
	import { untrack } from 'svelte';
	import type { SessionProfile } from '@bindings/SessionProfile';
	import type { OAuthAccount } from '$lib/queries';
	import ProfileList from './ProfileList.svelte';
	import type { ProfileSpecForm } from './profiles';

	let {
		profiles,
		accounts,
		selected = null,
		manageHref,
		oncreate,
		onsave,
		ondelete,
		onready
	}: {
		profiles: SessionProfile[];
		accounts: OAuthAccount[];
		selected?: string | null;
		manageHref?: string;
		oncreate: (name: string, spec: ProfileSpecForm) => Promise<boolean>;
		onsave: (id: string, name: string, spec: ProfileSpecForm) => void;
		ondelete: (id: string) => void;
		onready: (state: { selectedId: string | null; oneOff: ProfileSpecForm | null }) => void;
	} = $props();

	// svelte-ignore state_referenced_locally
	let state = $state<{ selectedId: string | null; oneOff: ProfileSpecForm | null }>({
		selectedId: selected,
		oneOff: null
	});
	untrack(() => onready(state));
</script>

<ProfileList
	{profiles}
	bind:selectedId={state.selectedId}
	bind:oneOff={state.oneOff}
	{accounts}
	pools={[{ id: 'pool1', name: 'shared', members: [] } as never]}
	usage={[]}
	machineId="m-uuid-1"
	{manageHref}
	{oncreate}
	{onsave}
	{ondelete}
/>
