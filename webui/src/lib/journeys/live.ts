/** A session a guide can open: in the registry and not dead. The book resolves
 *  `{fixture.session}` through this too, so neither host invents its own rule. */
export function isLive(s: { status: string; liveness?: string | null }): boolean {
	return (s.status === 'active' || s.status === 'new') && s.liveness !== 'dead';
}
