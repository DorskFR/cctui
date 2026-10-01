import { describe, expect, it } from 'vitest';
import { buildDispatchBody } from '$lib/components/organisms/spawn/dispatchBody';
import { contextPackEnv } from '$lib/components/organisms/spawn/options';
import type { Form } from '$lib/components/organisms/spawn/types';
import { parityFixture } from './fixtures';

type Pack = Partial<{ url: string; ref: string; subdir: string; token: string }>;

type Fixture = {
	contextPackEnv: { pack: Pack; out: Record<string, string> }[];
	buildDispatchBody: {
		why: string;
		form: Partial<Form>;
		env: Record<string, string>;
		pack: Pack;
		provider: string | null;
		sessionId: string;
		out: unknown;
	}[];
};

const fx = parityFixture<Fixture>('dispatchBody');

/** The fixture names the context pack as its own object; the webui form keeps
 *  the four fields flat, so they are spread in here. */
const form = (p: Partial<Form>, pack: Pack): Form =>
	({
		dispatcher: '',
		dispatch_adapter: '',
		name: '',
		identity: '',
		repo: '',
		ticket: '',
		prompt: '',
		prompt_file: '',
		model_claude: '',
		model_codex: '',
		model_account: '',
		effort_claude: '',
		effort_codex: '',
		timeout: '',
		account: '',
		context_pack_url: pack.url ?? '',
		context_pack_ref: pack.ref ?? '',
		context_pack_subdir: pack.subdir ?? '',
		context_pack_token: pack.token ?? '',
		...p
	}) as Form;

describe('dispatch body parity fixtures', () => {
	it('contextPackEnv', () => {
		for (const c of fx.contextPackEnv) {
			expect(contextPackEnv(form({}, c.pack)), JSON.stringify(c)).toEqual(c.out);
		}
	});

	it('buildDispatchBody', () => {
		for (const c of fx.buildDispatchBody) {
			const got = buildDispatchBody(form(c.form, c.pack), c.env, c.provider ?? undefined, c.sessionId);
			expect(JSON.parse(JSON.stringify(got)), c.why).toEqual(c.out);
		}
	});
});
