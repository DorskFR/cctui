import { getConfig } from './cctuiverse';
import { hasFeature } from './features.svelte';

export const cctuiverseConfig = $state({ enabled: false });

let loading: Promise<void> | null = null;

export function loadCctuiverseConfig(): Promise<void> {
	if (!hasFeature('federation')) return Promise.resolve();
	loading ??= getConfig()
		.then((c) => {
			cctuiverseConfig.enabled = c.enabled;
		})
		.catch(() => {
			loading = null;
		});
	return loading;
}
