// The server's domain tables, held for the synchronous lookups the render path
// needs (end tones, provider families). `useDomainMeta` fills them; until it
// resolves the lookups answer with the neutral/identity value rather than a
// second copy of the rules.
import type { DomainMeta } from '@bindings/DomainMeta';
import type { EndReasonInfo } from '@bindings/EndReasonInfo';
import type { ProviderInfo } from '@bindings/ProviderInfo';
import type { SessionEndReason } from '@bindings/SessionEndReason';

let meta = $state<DomainMeta | null>(null);

export function setDomainMeta(next: DomainMeta): void {
	meta = next;
}

export function domainMeta(): DomainMeta | null {
	return meta;
}

export function endReasonInfo(reason: SessionEndReason): EndReasonInfo | undefined {
	return meta?.end_reasons.find((r) => r.reason === reason);
}

export function providerInfo(id: string): ProviderInfo | undefined {
	return meta?.providers.find((p) => p.id === id);
}

export function providerKinds(): ProviderInfo[] {
	return meta?.providers ?? [];
}

export function usageProbes(): { id: string; label: string }[] {
	return meta?.usage_probes ?? [];
}
