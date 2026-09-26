import type { ComposerBridge } from './types';

// The drawer registers its composer per session; a pane reaches it through a
// stable handle so it never holds the component instance itself (the drawer
// remounts on session switches, the handle keeps working).
const bridges = new Map<string, ComposerBridge>();

export function registerComposer(sessionId: string, bridge: ComposerBridge): () => void {
	bridges.set(sessionId, bridge);
	return () => {
		if (bridges.get(sessionId) === bridge) bridges.delete(sessionId);
	};
}

export function composerFor(sessionId: string): ComposerBridge {
	const get = () => bridges.get(sessionId);
	return {
		insertText: (text) => get()?.insertText(text),
		addFiles: (files) => get()?.addFiles(files),
		focus: () => get()?.focus()
	};
}
