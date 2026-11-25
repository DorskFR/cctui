/** `dev` builds never match a server, so they never ask for a reload. */
export function needsReload(ui: string, srv: string | undefined, deployed: boolean): boolean {
	if (deployed) return true;
	if (!srv || ui === 'dev') return false;
	return ui !== srv;
}
