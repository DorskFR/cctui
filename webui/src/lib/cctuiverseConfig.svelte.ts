import { getConfig } from './cctuiverse';

export const cctuiverseConfig = $state({ enabled: false });

let loading: Promise<void> | null = null;

export function loadCctuiverseConfig(): Promise<void> {
	loading ??= getConfig()
		.then((c) => {
			cctuiverseConfig.enabled = c.enabled;
		})
		.catch(() => {
			loading = null;
		});
	return loading;
}
