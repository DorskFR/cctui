import { readFileSync } from 'node:fs';
import { afterEach, describe, expect, it } from 'vitest';
import { mount, unmount } from 'svelte';
import PermissionModeBadge from './PermissionModeBadge.svelte';
import { permissionTone } from '$lib/permissionMode';

let comp: ReturnType<typeof mount> | null = null;

afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

function render(mode?: string | null) {
	comp = mount(PermissionModeBadge, { target: document.body, props: { mode } });
	return document.body.querySelector('[data-testid="permission-mode"]');
}

describe('permission posture badge', () => {
	it('shows the posture the session runs under', () => {
		expect(render('yolo')?.textContent?.trim()).toBe('yolo');
		expect(render('plan')?.textContent?.trim()).toBe('plan');
	});

	it('normalizes the posture the server persisted', () => {
		expect(render('  Plan ')?.textContent?.trim()).toBe('plan');
	});

	it('renders nothing when the posture is unknown', () => {
		expect(render(null)).toBeNull();
		expect(render(undefined)).toBeNull();
		expect(render('   ')).toBeNull();
	});

	it('flags a permissionless posture as dangerous, planning as informational', () => {
		expect(permissionTone('yolo')).toBe('danger');
		expect(permissionTone('bypassPermissions')).toBe('danger');
		expect(permissionTone('plan')).toBe('info');
		expect(permissionTone('default')).toBe('neutral');
		expect(permissionTone(null)).toBe('neutral');
	});
});

describe('drawer header', () => {
	const header = readFileSync(
		'src/lib/components/organisms/conversation/DrawerHeader.svelte',
		'utf8'
	);

	it('shows the posture in the header meta row', () => {
		expect(header).toContain(
			"import PermissionModeBadge from '$lib/components/molecules/PermissionModeBadge.svelte';"
		);
		const trail = header.slice(header.indexOf('<div class="meta-trail">'));
		expect(trail).toContain('<PermissionModeBadge mode={session.permission_mode} />');
	});
});
