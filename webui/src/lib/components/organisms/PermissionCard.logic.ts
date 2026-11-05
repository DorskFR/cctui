export interface PermissionArgs {
	command: string | null;
	description: string | null;
	fields: [string, string][];
}

const COMMAND_KEYS = ['command', 'cmd'];

/** A tool-input JSON preview as readable parts; `null` when it is not a JSON object. */
export function permissionArgs(preview: string | null | undefined): PermissionArgs | null {
	if (!preview) return null;
	let obj: unknown;
	try {
		obj = JSON.parse(preview);
	} catch {
		return null;
	}
	if (!obj || typeof obj !== 'object' || Array.isArray(obj)) return null;
	let command: string | null = null;
	let description: string | null = null;
	const fields: [string, string][] = [];
	for (const [k, v] of Object.entries(obj as Record<string, unknown>)) {
		if (command === null && COMMAND_KEYS.includes(k) && typeof v === 'string') command = v;
		else if (description === null && k === 'description' && typeof v === 'string') description = v;
		else if (v !== null && v !== undefined) fields.push([k, typeof v === 'string' ? v : JSON.stringify(v)]);
	}
	return { command, description, fields };
}
