// The parts of `GET /meta/domain` only the server can know: today the
// quota-probe registry. The closed tables live in `$lib/domainTables`, so the
// render path never waits on this.
import type { DomainMeta } from '@bindings/DomainMeta';

let meta = $state<DomainMeta | null>(null);

export function setDomainMeta(next: DomainMeta): void {
	meta = next;
}

export function domainMeta(): DomainMeta | null {
	return meta;
}

export function usageProbes(): { id: string; label: string }[] {
	return meta?.usage_probes ?? [];
}
