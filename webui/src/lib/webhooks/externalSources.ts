import type { LaunchPreset } from '@bindings/LaunchPreset';
import type { LaunchPresetRequest } from '@bindings/LaunchPresetRequest';

export type ExternalSourcesOffer =
	| { kind: 'open' }
	| { kind: 'enable'; machine: boolean }
	| { kind: 'unguarded' };

type OfferInput = Pick<LaunchPreset, 'external_sources' | 'machine_id' | 'permission_mode'>;

export function externalSourcesOffer(preset: OfferInput): ExternalSourcesOffer {
	if (preset.external_sources) return { kind: 'open' };
	if (!preset.machine_id) return { kind: 'enable', machine: false };
	const guarded = preset.permission_mode === 'ask' || preset.permission_mode === 'auto';
	return guarded ? { kind: 'enable', machine: true } : { kind: 'unguarded' };
}

const RESPONSE_ONLY = ['id', 'user_id', 'next_fire_at', 'last_fire_at', 'created_at', 'updated_at'] as const;

export function withExternalSources(preset: LaunchPreset): LaunchPresetRequest {
	const body: Record<string, unknown> = { ...preset, external_sources: true };
	for (const key of RESPONSE_ONLY) delete body[key];
	return body as unknown as LaunchPresetRequest;
}

export interface PresetIo {
	get: (id: string) => Promise<LaunchPreset>;
	put: (id: string, body: LaunchPresetRequest) => Promise<LaunchPreset>;
}

export async function enableExternalSources(id: string, io: PresetIo): Promise<LaunchPreset> {
	const current = await io.get(id);
	if (current.external_sources) return current;
	return io.put(id, withExternalSources(current));
}
