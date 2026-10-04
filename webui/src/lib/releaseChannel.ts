export type ReleaseChannel = 'stable' | 'beta';

/** Any semver pre-release (`0.21.0-beta.1`) is a beta build, as on the server. */
export function releaseChannel(version: string): ReleaseChannel {
	return /^\d+\.\d+\.\d+$/.test(version.replace(/\+.*$/, '')) ? 'stable' : 'beta';
}

/** One line for both builds: `v0.24.0` when the UI and the server agree,
 *  `ui v… · srv v…` while a fresh UI waits for its server (or the reverse). */
export function versionLine(ui: string, srv: string | undefined): string {
	if (!srv || srv === ui) return `v${ui}`;
	return `ui v${ui} · srv v${srv}`;
}
