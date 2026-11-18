import type { HiddenOwner } from '@bindings/HiddenOwner';
import type { HiddenOwnerKind } from '@bindings/HiddenOwnerKind';
import type { MachineRow } from '@bindings/MachineRow';
import type { UserRow } from '@bindings/UserRow';
import { canHideUser, isHiddenOwner } from '#lib/hiddenOwners.ts';

export interface VisibilityMachine {
	id: string;
	name: string;
	shown: boolean;
}

export interface VisibilityRow {
	id: string;
	name: string;
	/** `null` for the admin's own user, which cannot be left out. */
	shown: boolean | null;
	machines: VisibilityMachine[];
}

export function visibilityRows(
	users: readonly UserRow[] | undefined,
	machines: readonly MachineRow[] | undefined,
	owners: readonly HiddenOwner[] | undefined,
	meId: string | null | undefined
): VisibilityRow[] {
	return (users ?? [])
		.filter((u) => !u.revoked_at)
		.map((u) => ({
			id: u.id,
			name: u.name,
			shown: canHideUser(meId, u.id) ? !isHiddenOwner(owners, 'user', u.id) : null,
			machines: (machines ?? [])
				.filter((mc) => mc.user_id === u.id && !mc.revoked_at)
				.map((mc) => ({
					id: mc.id,
					name: mc.display_name ?? mc.name,
					shown: !isHiddenOwner(owners, 'machine', mc.id)
				}))
				.sort((a, b) => a.name.localeCompare(b.name))
		}))
		.sort((a, b) => a.name.localeCompare(b.name));
}

type HideApi = {
	hide: (kind: HiddenOwnerKind, targetId: string, label?: string | null) => Promise<void>;
	unhide: (kind: HiddenOwnerKind, targetId: string) => Promise<void>;
};

/** The switch reads "show": OFF writes the owner to the admin's hidden list. */
export function setShown(
	api: HideApi,
	kind: HiddenOwnerKind,
	targetId: string,
	label: string,
	shown: boolean
): Promise<void> {
	return shown ? api.unhide(kind, targetId) : api.hide(kind, targetId, label);
}
