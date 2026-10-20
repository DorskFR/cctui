// The quota-probe registry, the one domain table only the server knows. The
// closed tables live in `$lib/domainTables`, so the render path never waits
// on this.
import type { UsageProbeInfo } from '@bindings/UsageProbeInfo';

let probes = $state<UsageProbeInfo[]>([]);

export function setUsageProbes(next: UsageProbeInfo[]): void {
	probes = next;
}

export function usageProbes(): UsageProbeInfo[] {
	return probes;
}
