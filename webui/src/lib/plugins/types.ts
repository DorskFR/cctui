import type { IconName } from '@dorsk/tsumikit';

export type {
	CctuiPluginModule,
	ComposerBridge,
	MessageAction,
	PageProps as PluginPageProps,
	PaneProps as PluginPaneProps,
	PluginMessage,
	PluginSession
} from '../../../plugin-sdk/types';
export {
	CCTUI_PLUGIN_API,
	CCTUI_PLUGIN_API_MINOR,
	HOST_CONTEXT_KEY,
	type HostContext
} from '../../../plugin-sdk/types';

export type { PluginInfo } from '$lib/bindings/PluginInfo';
export type { PluginPage } from '$lib/bindings/PluginPage';
export type { PluginSetting as PluginSettingDecl } from '$lib/bindings/PluginSetting';

/** One toolbar toggle per enabled pane plugin. */
export interface PluginButton {
	id: string;
	label: string;
	icon: IconName;
	open: boolean;
	onselect: () => void;
}

/** A message action resolved against the plugin that contributed it. */
export interface PluginActionButton {
	pluginId: string;
	label: string;
	icon: IconName;
	params: Record<string, string>;
	autoOpen: boolean;
}
