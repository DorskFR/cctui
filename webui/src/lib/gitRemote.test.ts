import { describe, expect, it } from 'vitest';
import { parseGithubRemote } from './gitRemote';

describe('parseGithubRemote', () => {
	it('parses the scp-style ssh remote', () => {
		expect(parseGithubRemote('git@github.com:DorskFR/cctui.git')).toEqual({
			owner: 'DorskFR',
			repo: 'cctui'
		});
	});

	it('parses an ssh remote without the .git suffix', () => {
		expect(parseGithubRemote('git@github.com:DorskFR/cctui')).toEqual({
			owner: 'DorskFR',
			repo: 'cctui'
		});
	});

	it('parses the ssh:// url form', () => {
		expect(parseGithubRemote('ssh://git@github.com/DorskFR/cctui.git')).toEqual({
			owner: 'DorskFR',
			repo: 'cctui'
		});
	});

	it('parses https with and without .git', () => {
		expect(parseGithubRemote('https://github.com/DorskFR/cctui.git')).toEqual({
			owner: 'DorskFR',
			repo: 'cctui'
		});
		expect(parseGithubRemote('https://github.com/DorskFR/cctui')).toEqual({
			owner: 'DorskFR',
			repo: 'cctui'
		});
	});

	it('tolerates a trailing slash and a credentialed https url', () => {
		expect(parseGithubRemote('https://github.com/DorskFR/cctui/')).toEqual({
			owner: 'DorskFR',
			repo: 'cctui'
		});
		expect(parseGithubRemote('https://x-access-token:tok@github.com/DorskFR/cctui.git')).toEqual({
			owner: 'DorskFR',
			repo: 'cctui'
		});
	});

	it('keeps a repo name that merely contains .git', () => {
		expect(parseGithubRemote('https://github.com/DorskFR/dot.github.git')).toEqual({
			owner: 'DorskFR',
			repo: 'dot.github'
		});
	});

	it('rejects non-GitHub hosts', () => {
		expect(parseGithubRemote('git@gitlab.com:DorskFR/cctui.git')).toBeNull();
		expect(parseGithubRemote('https://gitlab.com/DorskFR/cctui.git')).toBeNull();
		expect(parseGithubRemote('https://github.example.com/DorskFR/cctui.git')).toBeNull();
	});

	it('rejects empty, malformed and non-string input', () => {
		expect(parseGithubRemote(null)).toBeNull();
		expect(parseGithubRemote(undefined)).toBeNull();
		expect(parseGithubRemote('')).toBeNull();
		expect(parseGithubRemote('   ')).toBeNull();
		expect(parseGithubRemote('/srv/git/local.git')).toBeNull();
		expect(parseGithubRemote('https://github.com/DorskFR')).toBeNull();
	});
});
