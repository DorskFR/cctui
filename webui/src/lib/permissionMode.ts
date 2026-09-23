export type PermissionTone = 'danger' | 'info' | 'neutral';

export function normalizePermissionMode(mode?: string | null): string {
	return (mode ?? '').trim().toLowerCase();
}

export function permissionTone(mode?: string | null): PermissionTone {
	switch (normalizePermissionMode(mode)) {
		case 'yolo':
		case 'bypasspermissions':
			return 'danger';
		case 'plan':
			return 'info';
		default:
			return 'neutral';
	}
}
