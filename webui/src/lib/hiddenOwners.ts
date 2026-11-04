import type { HiddenOwner } from '@bindings/HiddenOwner';
import type { HiddenOwnerKind } from '@bindings/HiddenOwnerKind';
import type { MachineRow } from '@bindings/MachineRow';
import type { SessionListItem } from '@bindings/SessionListItem';
import type { UserRow } from '@bindings/UserRow';

export interface OwnerTarget {
	kind: HiddenOwnerKind;
	target_id: string;
	label: string;
	hidden: boolean;
}

export const isHiddenOwner = (
	owners: readonly HiddenOwner[] | undefined,
	kind: HiddenOwnerKind,
	targetId: string
): boolean => (owners ?? []).some((o) => o.kind === kind && o.target_id === targetId);

export const canHideUser = (meId: string | null | undefined, userId: string): boolean =>
	!!meId && meId !== userId;

export const isHiddenSession = (s: Pick<SessionListItem, 'hidden'>): boolean => s.hidden === true;

/** With Show hidden off the server already omits hidden sessions; this keeps a
 *  stale cache entry from leaking one back in. */
export const visibleSessions = <T extends Pick<SessionListItem, 'hidden'>>(
	sessions: readonly T[],
	showHidden: boolean
): T[] => (showHidden ? [...sessions] : sessions.filter((s) => !isHiddenSession(s)));

export const ownerLabel = (o: HiddenOwner): string => o.label || o.target_id;

export function sessionOwnerTargets(
	session: Pick<SessionListItem, 'machine_id' | 'machine_name'>,
	ctx: {
		meId: string | null | undefined;
		machines: readonly MachineRow[] | undefined;
		users: readonly UserRow[] | undefined;
		owners: readonly HiddenOwner[] | undefined;
	}
): OwnerTarget[] {
	const machine = (ctx.machines ?? []).find((mc) => mc.id === session.machine_id);
	const out: OwnerTarget[] = [];
	const ownerId = machine?.user_id;
	if (ownerId && canHideUser(ctx.meId, ownerId)) {
		const name = (ctx.users ?? []).find((u) => u.id === ownerId)?.name;
		out.push({
			kind: 'user',
			target_id: ownerId,
			label: name ?? ownerId,
			hidden: isHiddenOwner(ctx.owners, 'user', ownerId)
		});
	}
	out.push({
		kind: 'machine',
		target_id: session.machine_id,
		label: session.machine_name || machine?.display_name || machine?.name || session.machine_id,
		hidden: isHiddenOwner(ctx.owners, 'machine', session.machine_id)
	});
	return out;
}

export const sortedOwners = (owners: readonly HiddenOwner[] | undefined): HiddenOwner[] =>
	[...(owners ?? [])].sort(
		(a, b) => a.kind.localeCompare(b.kind) || ownerLabel(a).localeCompare(ownerLabel(b))
	);
