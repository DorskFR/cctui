import type { MachineRow } from '@bindings/MachineRow';
import type { MeResponse } from '@bindings/MeResponse';

/** Non-admins only ever see their own sessions; an admin owns one when its machine is theirs. */
export function ownsSession(
	me: MeResponse | undefined,
	machines: MachineRow[] | undefined,
	machineId: string
): boolean {
	if (!me) return false;
	if (me.role !== 'admin') return true;
	return (machines ?? []).some((mc) => mc.id === machineId);
}
