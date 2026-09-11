export interface GithubRemote {
	owner: string;
	repo: string;
}

const HOSTS = new Set(['github.com', 'www.github.com', 'ssh.github.com']);

function clean(segment: string): string {
	return segment.replace(/\.git$/, '').replace(/\/+$/, '');
}

export function parseGithubRemote(url: string | null | undefined): GithubRemote | null {
	if (typeof url !== 'string') return null;
	const raw = url.trim();
	if (!raw) return null;

	const scp = /^(?:([^@/]+)@)?([^:/]+):(?!\/)(.+)$/.exec(raw);
	const path = scp && HOSTS.has(scp[2].toLowerCase()) ? scp[3] : fromUrl(raw);
	if (path === null) return null;

	const parts = path.replace(/^\/+/, '').split('/').filter(Boolean);
	if (parts.length < 2) return null;
	const owner = clean(parts[0]);
	const repo = clean(parts[1]);
	if (!owner || !repo) return null;
	return { owner, repo };
}

function fromUrl(raw: string): string | null {
	try {
		const u = new URL(raw);
		if (!['http:', 'https:', 'ssh:', 'git:'].includes(u.protocol)) return null;
		if (!HOSTS.has(u.hostname.toLowerCase())) return null;
		return u.pathname;
	} catch {
		return null;
	}
}
