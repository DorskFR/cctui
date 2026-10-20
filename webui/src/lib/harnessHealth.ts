import type { HarnessReport } from '@bindings/HarnessReport';
import type { HarnessVersions } from '@bindings/HarnessVersions';
import type { MachineHealth } from '@bindings/MachineHealth';

export type VersionSkew = { harness: string; cli: string; daemon: string };

/** Harnesses whose long-lived process runs a different version than the CLI
 *  on disk: the daemon was not restarted after an auto-update. */
export function versionSkew(versions: HarnessVersions): VersionSkew[] {
	return Object.entries(versions).flatMap(([harness, v]) =>
		v.cli && v.daemon && v.cli !== v.daemon ? [{ harness, cli: v.cli, daemon: v.daemon }] : []
	);
}

export type DeniedHealth = Exclude<MachineHealth, { state: 'ok' }>;

/** The probe's denial, or `null` when healthy or never probed. */
export function deniedHealth(report: Pick<HarnessReport, 'health'> | null | undefined): DeniedHealth | null {
	const h = report?.health;
	return h && h.state !== 'ok' ? h : null;
}

/** Only claude-code's daemon can be drained from the card today. */
export const DRAIN_RESTARTABLE: ReadonlySet<string> = new Set(['claude-code']);
