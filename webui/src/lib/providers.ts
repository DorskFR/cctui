// Provider-id helpers shared by the accounts surfaces. The metadata is the
// parity table in `$lib/domainTables`; only the quota-probe registry, which is
// the server's alone, comes off the wire.
import type { ProviderFamily } from '@bindings/ProviderFamily';
import { PROVIDERS, providerInfo } from '$lib/domainTables';
import { usageProbes } from '$lib/domainMeta.svelte';

export type { ProviderFamily };
export type ProviderKind = string;

/** Display name for an account provider id; an unknown id stays itself. */
export const providerLabel = (p: string) => providerInfo(p)?.label ?? p;

export const providerFamily = (p: string): ProviderFamily =>
  providerInfo(p)?.family ?? 'anthropic';

export const isStaticCredential = (p: string) =>
  providerInfo(p)?.static_credential ?? false;

/** The selectable provider kinds, in the order the pickers list them. */
export const providerKindOptions = (): { value: string; label: string }[] =>
  PROVIDERS.map((p) => ({ value: p.id, label: p.picker_label }));

/** Quota probes the server's registry serves, for the `usage_probe` picker. */
export const usageProbeOptions = (): { value: string; label: string }[] =>
  usageProbes().map((p) => ({ value: p.id, label: p.label }));
