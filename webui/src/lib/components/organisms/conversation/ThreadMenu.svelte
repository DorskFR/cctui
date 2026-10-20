<script lang="ts">
	import type { SessionListItem } from '@bindings/SessionListItem';
	import { goto } from '$app/navigation';
	import { Badge, Icon, Menu, Text, relativeTime, type MenuItem } from '@dorsk/tsumikit';
	import { sessionHref } from '$lib/sessionFailureToast';
	import { useSessionThread } from '$lib/queries';
	import { m } from '$lib/paraglide/messages';
	import { threadEntries, type ThreadEntry } from './thread';

	let { session, box = 'md' }: { session: SessionListItem; box?: 'sm' | 'md' } = $props();

	const thread = useSessionThread(
		() => session.id,
		() => session.next_session_id ?? null
	);
	const entries = $derived(
		threadEntries(session, thread.data?.sessions, {
			previous: m.drawer_thread_previous(),
			next: m.drawer_thread_next()
		})
	);
	const byLabel = $derived(new Map(entries.map((e) => [e.label, e])));
	const items = $derived<MenuItem[]>(
		entries.map((e) => ({
			label: e.label,
			content: row,
			attrs: e.current ? { 'aria-current': 'page' } : {},
			onselect: () => {
				if (!e.current) void goto(sessionHref(e.id));
			}
		}))
	);
</script>

{#snippet row(item: MenuItem)}
	{@const e = byLabel.get(item.label) as ThreadEntry}
	<Text weight={e.current ? 'bold' : 'normal'} truncate>{e.title}</Text>
	{#if e.at}<Badge size="xs" border={false}>{relativeTime(e.at, undefined, false, true)}</Badge>{/if}
{/snippet}

{#if entries.length > 0}
	<span class="thread" data-journey="head-thread">
		<Menu label={m.drawer_thread_menu()} {items} placement="bottom-start" {box} variant="ghost">
			{#snippet trigger()}<Icon name="list" size={18} />{/snippet}
		</Menu>
	</span>
{/if}

<style>
	.thread {
		display: inline-flex;
		flex: none;
	}
</style>
