import type { MachineAdapterInfo } from '@bindings/MachineAdapterInfo';
import type { MachineResourcesRow } from '@bindings/MachineResourcesRow';
import type { MachineRow } from '@bindings/MachineRow';

export type BinaryStatus = 'found' | 'missing' | 'unknown';

export function binaryStatus(a: MachineAdapterInfo): BinaryStatus {
	if (!a.binary) return 'unknown';
	return a.binary.path ? 'found' : 'missing';
}

export type ParsedConfig =
	| { ok: true; config: Record<string, unknown> }
	| { ok: false; reason: 'syntax'; detail: string }
	| { ok: false; reason: 'not_object' };

export function parseAdapterConfig(text: string): ParsedConfig {
	if (!text.trim()) return { ok: true, config: {} };
	let value: unknown;
	try {
		value = JSON.parse(text);
	} catch (e) {
		return { ok: false, reason: 'syntax', detail: e instanceof Error ? e.message : String(e) };
	}
	if (value === null || typeof value !== 'object' || Array.isArray(value)) {
		return { ok: false, reason: 'not_object' };
	}
	return { ok: true, config: value as Record<string, unknown> };
}

export const formatAdapterConfig = (config: Record<string, unknown>): string =>
	JSON.stringify(config ?? {}, null, 2);

export interface AdapterMachine {
	id: string;
	name: string;
}

export function adapterMachines(
	isAdmin: boolean,
	all: readonly MachineRow[] | undefined,
	own: readonly MachineResourcesRow[] | undefined
): AdapterMachine[] {
	if (isAdmin) return (all ?? []).map((r) => ({ id: r.id, name: r.display_name ?? r.name }));
	return (own ?? []).map((r) => ({ id: r.machine_id, name: r.display_name ?? r.name }));
}
