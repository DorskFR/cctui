// The admin token for the local stack: CCTUI_TOKEN, else the first token that
// `make local/up` generated into deploy/local/.env.
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const envFile = resolve(dirname(fileURLToPath(import.meta.url)), '../../deploy/local/.env');

export function localToken() {
	if (process.env.CCTUI_TOKEN) return process.env.CCTUI_TOKEN;
	let text = '';
	try {
		text = readFileSync(envFile, 'utf8');
	} catch {
		// fall through to the error below
	}
	const line = text.split('\n').find((l) => l.startsWith('CCTUI_ADMIN_TOKENS='));
	const token = line?.slice('CCTUI_ADMIN_TOKENS='.length).split(',')[0].trim();
	if (!token) {
		throw new Error(`no admin token: set CCTUI_TOKEN or run \`make local/up\` to generate ${envFile}`);
	}
	return token;
}
