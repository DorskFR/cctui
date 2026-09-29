import { browser } from '$app/environment';
import { CCTUI_PLUGIN_API, CCTUI_PLUGIN_API_MINOR, type HostContext } from './types';

export function hostContext(): HostContext {
	return {
		cctuiApi: CCTUI_PLUGIN_API,
		cctuiApiMinor: CCTUI_PLUGIN_API_MINOR,
		origin: browser ? location.origin : ''
	};
}
