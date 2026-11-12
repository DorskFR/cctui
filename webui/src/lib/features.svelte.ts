import type { Feature } from '@bindings/Feature';
import type { CapabilitiesResponse } from '@bindings/CapabilitiesResponse';

type Announced = Pick<CapabilitiesResponse, 'features' | 'switchable'>;

let announced = $state<{ on: Set<string>; switchable: Set<string> } | null>(null);

function adopt(caps: Partial<Announced> | null | undefined): void {
	announced = caps
		? { on: new Set(caps.features ?? []), switchable: new Set(caps.switchable ?? []) }
		: null;
}

/** On when announced in `features`, or unknown to the server (absent from `switchable`).
 *  Before capabilities have loaded, and when the probe fails, every feature counts as on. */
export function hasFeature(name: Feature): boolean {
	const a = announced;
	return !a || a.on.has(name) || !a.switchable.has(name);
}

/** False when the route belongs to a feature that is off. */
export function routeAvailable(route: { feature: Feature | null }): boolean {
	return route.feature === null || hasFeature(route.feature);
}

export function setCapabilities(caps: Partial<CapabilitiesResponse> | null): void {
	adopt(caps);
}

/** Tests only. `null` restores "not loaded". */
export function setCapabilitiesForTest(caps: Announced | null): void {
	adopt(caps);
}
