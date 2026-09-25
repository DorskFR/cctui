export type ReleaseChannel = 'stable' | 'beta';

/** Any semver pre-release (`0.21.0-beta.1`) is a beta build, as on the server. */
export function releaseChannel(version: string): ReleaseChannel {
	return /^\d+\.\d+\.\d+$/.test(version.replace(/\+.*$/, '')) ? 'stable' : 'beta';
}
