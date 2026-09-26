<script lang="ts">
	import { errMessage } from '$lib/api';
	import { useToolPolicy, useToolPolicyActions, type OAuthAccount } from '$lib/queries';
	import { toasts } from '$lib/toast.svelte';
	import { Badge, Button, Card, Field, Stack, Textarea } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import {
		fromDraft,
		policyActive,
		sameDraft,
		toDraft,
		type PolicyDraft
	} from './tool-policy.logic';

	let { account }: { account: OAuthAccount } = $props();

	const policy = useToolPolicy(() => account.id);
	const actions = useToolPolicyActions();
	const stored = $derived(toDraft(policy.data));
	let edits = $state<Partial<PolicyDraft>>({});
	let saving = $state(false);

	const draft = $derived<PolicyDraft>({ ...stored, ...edits });
	const dirty = $derived(!sameDraft(draft, stored));
	const active = $derived(!!policy.data && policyActive(policy.data));

	const fields = $derived([
		{
			key: 'terms',
			label: m.tool_policy_terms(),
			help: m.tool_policy_terms_help(),
			placeholder: 'acme\nproject-falcon'
		},
		{
			key: 'patterns',
			label: m.tool_policy_patterns(),
			help: m.tool_policy_patterns_help(),
			placeholder: 'ACME-[0-9]{4,}'
		},
		{
			key: 'protected_owners',
			label: m.tool_policy_owners(),
			help: m.tool_policy_owners_help(),
			placeholder: 'acme-corp'
		},
		{
			key: 'exempt_roots',
			label: m.tool_policy_exempt(),
			help: m.tool_policy_exempt_help(),
			placeholder: '/home/me/work'
		}
	] as const);

	async function save() {
		saving = true;
		try {
			await actions.put(account.id, fromDraft(draft));
			edits = {};
			toasts.ok(m.tool_policy_saved());
		} catch (e) {
			toasts.error(errMessage(e));
		} finally {
			saving = false;
		}
	}
</script>

<Card title={m.tool_policy_title()} subtitle={m.tool_policy_help()}>
	{#snippet actions()}
		<Badge tone={active ? 'accent' : 'neutral'}>
			{active ? m.tool_policy_on() : m.tool_policy_off()}
		</Badge>
		<Button size="sm" variant="primary" disabled={!dirty || saving} onclick={save}>
			{m.common_save()}
		</Button>
	{/snippet}

	<Stack>
		{#each fields as f (f.key)}
			<Field label={f.label} hint={f.help}>
				<Textarea
					mono
					autoresize
					rows={3}
					value={draft[f.key]}
					placeholder={f.placeholder}
					oninput={(e) =>
						(edits = { ...edits, [f.key]: (e.currentTarget as HTMLTextAreaElement).value })}
				/>
			</Field>
		{/each}
	</Stack>
</Card>
