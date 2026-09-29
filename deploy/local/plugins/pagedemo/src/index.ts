import type { CctuiPluginModule } from '../../../../../webui/plugin-sdk/types';
import Page from './Page.svelte';

const plugin: CctuiPluginModule = {
	cctuiApi: 1,
	page: Page
};

export default plugin;
