const RELEASE = /^\d+\.\d+\.\d+(-[0-9A-Za-z.]+)?$/;

/** Only a release ui can match a server; `dev`, `pr-<n>` and `-dirty` builds never ask. */
export function needsReload(ui: string, srv: string | undefined, deployed: boolean): boolean {
	if (deployed) return true;
	if (!srv || !RELEASE.test(ui) || ui.endsWith('-dirty')) return false;
	return ui !== srv;
}
