import type { CctuiPluginModule } from '../../../../plugin-sdk/types';
import DemoPane from './DemoPane.svelte';

const plugin: CctuiPluginModule = {
	cctuiApi: 1,
	sessionPane: DemoPane,
	messageActions: (msg) => {
		const m = /\btest\b/i.exec(msg.text);
		return m ? [{ label: 'Open in Demo', icon: 'eye', params: { word: m[0] }, open: 'sessionPane', autoOpen: true }] : [];
	}
};

export default plugin;
