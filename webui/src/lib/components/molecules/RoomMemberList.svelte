<script lang="ts">
	import { Badge, Button, EmptyState, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import { behindBy, isDormant, memberLabel, type RoomMember } from '$lib/rooms';

	let {
		members,
		head = 0,
		busy = false,
		onRemove
	}: {
		members: RoomMember[];
		/** The room's newest seq, so a lagging member can be marked. */
		head?: number;
		busy?: boolean;
		onRemove?: (m: RoomMember) => void;
	} = $props();

	const ROW = 'justify-content:flex-start;text-align:left;min-width:0;font-size:var(--fs-sm)';
</script>

{#if members.length === 0}
	<EmptyState title={m.rooms_members_count({ count: 0 })} />
{:else}
	<ul class="members">
		{#each members as mem (mem.session_id)}
			{@const behind = behindBy(head, mem)}
			<li class="row" class:dormant={isDormant(mem)}>
				<span class="who" title={memberLabel(mem)}>
					<Text size="sm">{memberLabel(mem)}</Text>
				</span>
				{#if mem.role === 'observer'}
					<Badge size="xs">{m.rooms_member_observer()}</Badge>
				{/if}
				{#if isDormant(mem)}
					<Badge size="xs" uppercase>{mem.state}</Badge>
				{/if}
				{#if behind > 0}
					<Badge size="xs" title={m.rooms_member_behind({ count: behind })}>
						{m.rooms_member_behind({ count: behind })}
					</Badge>
				{/if}
				{#if onRemove}
					<Button
						variant="ghost"
						size="sm"
						style={ROW}
						disabled={busy}
						title={m.rooms_member_remove()}
						aria-label={m.rooms_member_remove()}
						onclick={() => onRemove?.(mem)}>×</Button
					>
				{/if}
			</li>
		{/each}
	</ul>
{/if}

<style>
	.members {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
		margin: 0;
		padding: 0;
		list-style: none;
	}
	.row {
		display: flex;
		gap: var(--sp-2);
		align-items: center;
		min-width: 0;
	}
	/* An ended or archived member stays listed but reads as unreachable. */
	.dormant {
		opacity: 0.55;
	}
	.who {
		min-width: 0;
		overflow: hidden;
		white-space: nowrap;
		text-overflow: ellipsis;
	}
</style>
