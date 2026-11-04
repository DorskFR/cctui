<script lang="ts">
	import { errMessage } from '$lib/api';
	import { useToolPolicy, useToolPolicyActions, type OAuthAccount } from '$lib/queries';
	import { toasts } from '$lib/toast.svelte';
	import {
		Badge,
		Button,
		Card,
		Cluster,
		Field,
		IconButton,
		Input,
		Select,
		Stack,
		Textarea
	} from '@dorsk/tsumikit';
	import type { HookPolicyEvent as HookEvent } from '@bindings/HookPolicyEvent';
	import type { ToolClass } from '@bindings/ToolClass';
	import { m } from '$lib/paraglide/messages';
	import {
		HOOK_EVENTS,
		TOOL_CLASSES,
		blankHook,
		fromDraft,
		hooksOf,
		policyActive,
		sameDraft,
		sameHooks,
		takesTool,
		toDraft,
		toHookDrafts,
		type HookDraft,
		type PolicyDraft
	} from './tool-policy.logic';

	let { account }: { account: OAuthAccount } = $props();

	const policy = useToolPolicy(() => account.id);
	const actions = useToolPolicyActions();
	const stored = $derived(toDraft(policy.data));
	const storedHooks = $derived(toHookDrafts(hooksOf(policy.data)));
	let edits = $state<Partial<PolicyDraft>>({});
	let hookEdits = $state<HookDraft[] | null>(null);
	let saving = $state(false);

	const draft = $derived<PolicyDraft>({ ...stored, ...edits });
	const hooks = $derived(hookEdits ?? storedHooks);
	const dirty = $derived(!sameDraft(draft, stored) || !sameHooks(hooks, storedHooks));
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

	const toolLabels: Record<ToolClass, () => string> = {
		shell: m.hook_policy_tool_shell,
		file_edit: m.hook_policy_tool_file_edit,
		file_write: m.hook_policy_tool_file_write,
		subagent: m.hook_policy_tool_subagent,
		any: m.hook_policy_tool_any
	};
	const eventOptions = HOOK_EVENTS.map((e) => ({ value: e, label: e }));
	const toolOptions = $derived(TOOL_CLASSES.map((t) => ({ value: t, label: toolLabels[t]() })));

	function setHook(i: number, patch: Partial<HookDraft>) {
		hookEdits = hooks.map((h, j) => (j === i ? { ...h, ...patch } : h));
	}

	async function save() {
		saving = true;
		try {
			await actions.put(account.id, fromDraft(draft, hooks));
			edits = {};
			hookEdits = null;
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
		<Field label={m.hook_policy_title()} hint={m.hook_policy_help()}>
			<Stack gap="var(--sp-2)" data-testid="hook-policy-editor">
				{#each hooks as row, i (i)}
					<Cluster gap="var(--sp-2)" wrap data-testid="hook-policy-row">
						<Select
							size="sm"
							width="auto"
							value={row.event}
							options={eventOptions}
							aria-label={m.hook_policy_event()}
							onchange={(e) => setHook(i, { event: e.currentTarget.value as HookEvent })}
						/>
						<Select
							size="sm"
							width="auto"
							value={takesTool(row.event) ? row.tool : 'any'}
							options={toolOptions}
							disabled={!takesTool(row.event)}
							aria-label={m.hook_policy_tool()}
							onchange={(e) => setHook(i, { tool: e.currentTarget.value as ToolClass })}
						/>
						<Input
							mono
							grow
							size="sm"
							value={row.command}
							placeholder="/opt/guards/deny-rm.sh"
							aria-label={m.hook_policy_command()}
							oninput={(e) => setHook(i, { command: e.currentTarget.value })}
						/>
						<Input
							size="sm"
							width="6rem"
							inputmode="numeric"
							value={row.timeout}
							placeholder={m.hook_policy_timeout()}
							aria-label={m.hook_policy_timeout()}
							oninput={(e) => setHook(i, { timeout: e.currentTarget.value })}
						/>
						<IconButton
							icon="trash"
							variant="ghost"
							label={m.hook_policy_remove()}
							onclick={() => (hookEdits = hooks.filter((_, j) => j !== i))}
						/>
					</Cluster>
				{/each}
				<Cluster>
					<Button size="sm" onclick={() => (hookEdits = [...hooks, blankHook()])}>
						{m.hook_policy_add()}
					</Button>
				</Cluster>
			</Stack>
		</Field>
	</Stack>
</Card>
