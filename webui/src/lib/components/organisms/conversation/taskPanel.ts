import { drafts } from '$lib/drafts';

export const taskPanelKey = (sessionId: string) => `cctui_tasks_open_${sessionId}`;

// Collapsed is the default, so only the expanded state is persisted — an absent
// key must read as closed.
export function taskPanelOpen(sessionId: string): boolean {
	return drafts.get(taskPanelKey(sessionId)) === '1';
}

export function setTaskPanelOpen(sessionId: string, open: boolean) {
	drafts.set(taskPanelKey(sessionId), open ? '1' : '');
}
