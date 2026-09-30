import type { SpawnPrefill } from '$lib/components/organisms/spawn/types';
import type { HostSpawnRequest } from './types';

/** A plugin's `openSpawn`: the host layout mounts its SpawnModal while a request
 *  is pending, so a plugin can hand the user a prepared session from any route.
 *  The user still submits the form — a plugin never launches anything itself. */
class PluginSpawn {
	prefill = $state<SpawnPrefill | null>(null);

	open(req: HostSpawnRequest) {
		this.prefill = {
			prompt: req.prompt,
			...(req.working_dir ? { working_dir: req.working_dir } : {}),
			...(req.machine_id ? { machine_id: req.machine_id } : {})
		};
	}

	close() {
		this.prefill = null;
	}
}

export const pluginSpawn = new PluginSpawn();
