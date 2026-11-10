import type { Feature } from './bindings/Feature';
import { ROUTES } from './bindings/routes';

/** Switchable features that gate no HTTP route, only a socket message. */
const ROUTELESS: Feature[] = ['terminal'];

export const ALL_FEATURES: Feature[] = [
	...new Set([...ROUTES.flatMap((r): Feature[] => (r.feature ? [r.feature] : [])), ...ROUTELESS])
];
