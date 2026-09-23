import { afterEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import AutoArchiveNotice from './AutoArchiveNotice.svelte';

let comp: ReturnType<typeof mount> | null = null;

afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

type Props = {
	status: 'active' | 'inactive' | 'archived';
	liveness: 'active' | 'stale' | 'dead';
	auto_archive_at?: string | null;
	archived_by?: 'user' | 'automatic' | null;
};

function render(session: Props, onpin = () => {}): HTMLElement {
	const host = document.createElement('div');
	document.body.appendChild(host);
	comp = mount(AutoArchiveNotice, { target: host, props: { session, onpin } as never });
	return host;
}

const notice = (host: HTMLElement) => host.querySelector('[data-testid="auto-archive-notice"]');

describe('AutoArchiveNotice', () => {
	it('warns an idle session of its archive time and pins on click', () => {
		const onpin = vi.fn();
		const host = render(
			{ status: 'inactive', liveness: 'dead', auto_archive_at: '2026-09-25T10:00:00Z' },
			onpin
		);
		expect(notice(host)).not.toBeNull();
		host.querySelector('button')?.click();
		flushSync();
		expect(onpin).toHaveBeenCalledOnce();
	});

	it('stays silent while the session is working or has no due time', () => {
		expect(
			notice(render({ status: 'active', liveness: 'active', auto_archive_at: '2026-09-25T10:00:00Z' }))
		).toBeNull();
		unmount(comp!);
		comp = null;
		expect(notice(render({ status: 'inactive', liveness: 'dead', auto_archive_at: null }))).toBeNull();
	});

	it('says an archived session was archived automatically, not by the user', () => {
		const auto = render({ status: 'archived', liveness: 'dead', archived_by: 'automatic' });
		expect(notice(auto)).not.toBeNull();
		expect(auto.querySelector('button')).toBeNull();
		unmount(comp!);
		comp = null;
		expect(notice(render({ status: 'archived', liveness: 'dead', archived_by: 'user' }))).toBeNull();
	});
});
