import type { SessionListItem } from '@bindings/SessionListItem';

export function livenessClass(s: Pick<SessionListItem, 'hibernated' | 'liveness'>): string {
	if (s.hibernated) return 'dot-hibernated';
	if (s.liveness === 'active') return 'dot-active';
	if (s.liveness === 'stale') return 'dot-stale';
	return 'dot-dead';
}
