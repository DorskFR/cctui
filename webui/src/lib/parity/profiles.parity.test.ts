import { describe, expect, it } from 'vitest';
import type { AccountPoolView } from '@bindings/AccountPoolView';
import type { SessionProfile } from '@bindings/SessionProfile';
import type { OAuthAccount } from '$lib/queries';
import {
	accountPick,
	applySpec,
	initialProfile,
	modelField,
	moveProfile,
	moveProfileOnto,
	specChain,
	specChanges,
	specFromForm,
	uniqueProfileName,
	type ProfileSpecForm
} from '$lib/components/organisms/spawn/profiles';
import { parityFixture } from './fixtures';

type SpecForm = ProfileSpecForm;
type Form = Parameters<typeof specFromForm>[0];

type Fixture = {
	accounts: { id: string; name: string; emoji: string | null; providers: string[] }[];
	pools: { id: string; name: string }[];
	labels: {
		auto: string;
		noAccount: string;
		defaultModel: string;
		defaultEffort: string;
		defaultMode: string;
	};
	modelField: { harness: string; account_id: string | null; out: string }[];
	accountPick: { spec: SpecForm; out: string }[];
	specFromForm: { form: Form; out: SpecForm }[];
	applySpec: { form: Form; spec: SpecForm; out: Form }[];
	specChanges: { a: SpecForm; b: SpecForm; out: number }[];
	specChain: { spec: SpecForm; out: string }[];
	uniqueProfileName: { base: string; existing: string[]; out: string }[];
	initialProfile: { ids: string[]; last_used: string | null; out: string | null }[];
	moveProfile: { ids: string[]; id: string; index: number; out: string[] }[];
	moveProfileOnto: { ids: string[]; id: string; target: string; out: string[] }[];
};

const fx = parityFixture<Fixture>('profiles');

const accounts = fx.accounts.map(
	(a) =>
		({
			id: a.id,
			name: a.name,
			emoji: a.emoji,
			providers: a.providers.map((provider) => ({ provider }))
		}) as unknown as OAuthAccount
);
const pools = fx.pools.map((p) => ({ ...p, members: [] }) as unknown as AccountPoolView);
const byId = (id: string | null) => accounts.find((a) => a.id === id);
const profileOf = (id: string) => ({ id }) as unknown as SessionProfile;

describe('spawn profile parity fixtures', () => {
	it('modelField', () => {
		for (const c of fx.modelField)
			expect(modelField(c.harness, byId(c.account_id)), JSON.stringify(c)).toBe(c.out);
	});
	it('accountPick', () => {
		for (const c of fx.accountPick)
			expect(accountPick(c.spec, accounts, pools), JSON.stringify(c)).toBe(c.out);
	});
	it('specFromForm', () => {
		for (const c of fx.specFromForm)
			expect(specFromForm(c.form, accounts, pools), JSON.stringify(c)).toEqual(c.out);
	});
	it('applySpec', () => {
		for (const c of fx.applySpec)
			expect(applySpec(c.form, c.spec, accounts, pools), JSON.stringify(c)).toEqual(c.out);
	});
	it('specChanges', () => {
		for (const c of fx.specChanges)
			expect(specChanges(c.a, c.b), JSON.stringify(c)).toBe(c.out);
	});
	it('specChain', () => {
		for (const c of fx.specChain)
			expect(
				specChain(c.spec, accounts, pools, fx.labels, (_h, alias) => alias),
				JSON.stringify(c)
			).toBe(c.out);
	});
	it('uniqueProfileName', () => {
		for (const c of fx.uniqueProfileName)
			expect(uniqueProfileName(c.base, c.existing), JSON.stringify(c)).toBe(c.out);
	});
	it('initialProfile', () => {
		for (const c of fx.initialProfile)
			expect(
				initialProfile(c.ids.map(profileOf), c.last_used)?.id ?? null,
				JSON.stringify(c)
			).toBe(c.out);
	});
	it('moveProfile', () => {
		for (const c of fx.moveProfile)
			expect(
				moveProfile(c.ids.map(profileOf), c.id, c.index).map((p) => p.id),
				JSON.stringify(c)
			).toEqual(c.out);
	});
	it('moveProfileOnto', () => {
		for (const c of fx.moveProfileOnto)
			expect(
				moveProfileOnto(c.ids.map(profileOf), c.id, c.target).map((p) => p.id),
				JSON.stringify(c)
			).toEqual(c.out);
	});
});
