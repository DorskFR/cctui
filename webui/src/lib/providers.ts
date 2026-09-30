// Provider-id helpers shared by the accounts surfaces. The metadata itself is
// the server's (`GET /meta/domain`); these only read it.
import type { ProviderFamily } from '@bindings/ProviderFamily';
import { providerInfo, providerKinds, usageProbes } from '$lib/domainMeta.svelte';

export type { ProviderFamily };
export type ProviderKind = string;

/** Display name for an account provider id; an id the server does not know
 *  stays itself. */
export const providerLabel = (p: string) => providerInfo(p)?.label ?? p;

export const providerFamily = (p: string): ProviderFamily =>
  providerInfo(p)?.family ?? 'anthropic';

export const isStaticCredential = (p: string) =>
  providerInfo(p)?.static_credential ?? false;

/** The selectable provider kinds, in the order the server lists them. */
export const providerKindOptions = (): { value: string; label: string }[] =>
  providerKinds().map((p) => ({ value: p.id, label: p.picker_label }));

/** Quota probes the server's registry serves, for the `usage_probe` picker. */
export const usageProbeOptions = (): { value: string; label: string }[] =>
  usageProbes().map((p) => ({ value: p.id, label: p.label }));
