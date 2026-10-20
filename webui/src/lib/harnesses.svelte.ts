// The harness table in effect: the rows `GET /harnesses` served, else the
// shipped `HARNESSES`, so pickers and gates answer before the query lands.
import type { HarnessDescriptor } from '@bindings/HarnessDescriptor';
import { HARNESSES } from '$lib/domainTables';

let served = $state<HarnessDescriptor[] | null>(null);

export function setHarnesses(next: HarnessDescriptor[]): void {
	served = next.length ? next : null;
}

export function harnessTable(): HarnessDescriptor[] {
	return served ?? HARNESSES;
}
