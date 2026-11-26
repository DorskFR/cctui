<script lang="ts">
	import { Callout, Link } from '@dorsk/tsumikit';
	import { useMe } from '#lib/queries/index.ts';
	import { settingsHref } from '../settings/settings.logic';
	import { m } from '#lib/paraglide/messages.js';

	const me = useMe();
	const isAdmin = $derived(me.data?.role === 'admin');
</script>

<Callout tone="warn" icon="info">
	{m.spawn_no_machines_hint()}
	{#if isAdmin}
		<Link href={settingsHref('instance', 'instance-users')}>{m.access_title()}</Link>
	{:else}
		<Link href={settingsHref('accounts', 'profile')}>{m.settings_sec_profile()}</Link>
	{/if}
</Callout>
