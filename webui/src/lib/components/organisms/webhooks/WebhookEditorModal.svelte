<script lang="ts">
	import type { LaunchPreset } from '@bindings/LaunchPreset';
	import type { Webhook } from '@bindings/Webhook';
	import type { WebhookSecret } from '@bindings/WebhookSecret';
	import { Button, Callout, Field, Input, Modal, Select, Switch, Text, Textarea } from '@dorsk/tsumikit';
	import type { SelectOption } from '@dorsk/tsumikit';
	import { errMessage } from '#lib/api.ts';
	import PresetEditorModal from '#lib/components/organisms/presets/PresetEditorModal.svelte';
	import { useWebhookActions } from '#lib/queries/webhooks.ts';
	import { m } from '#lib/paraglide/messages.js';
	import { templateExpressions } from '#lib/webhooks/template.ts';
	import {
		RATE_MAX,
		RATE_MIN,
		eligiblePresets,
		emptyForm,
		formFromWebhook,
		formProblems,
		hasProblems,
		requestFromForm,
		type WebhookForm
	} from '#lib/webhooks/webhookForm.ts';

	let {
		hook = null,
		presets,
		onclose,
		oncreated
	}: {
		hook?: Webhook | null;
		presets: LaunchPreset[];
		onclose: () => void;
		oncreated?: (created: WebhookSecret) => void;
	} = $props();

	const uid = $props.id();
	const actions = useWebhookActions();

	// svelte-ignore state_referenced_locally
	let form = $state<WebhookForm>(hook ? formFromWebhook(hook) : emptyForm());
	let added = $state<LaunchPreset[]>([]);
	let newPreset = $state(false);
	let tried = $state(false);
	let busy = $state(false);
	let failure = $state('');

	const problems = $derived(formProblems(form));
	const expressions = $derived(templateExpressions(form.prompt_template));
	const presetOptions = $derived.by<SelectOption[]>(() => {
		const known = new Map([...presets, ...added].map((p) => [p.id, p]));
		const out: SelectOption[] = eligiblePresets([...known.values()]).map((p) => ({
			value: p.id,
			label: p.name
		}));
		if (!form.preset_id) out.unshift({ value: '', label: m.webhooks_preset_pick() });
		else if (!out.some((o) => o.value === form.preset_id)) {
			out.unshift({
				value: form.preset_id,
				label: known.get(form.preset_id)?.name ?? m.webhooks_preset_deleted()
			});
		}
		return out;
	});

	async function save() {
		tried = true;
		if (hasProblems(problems) || busy) return;
		busy = true;
		failure = '';
		try {
			const body = requestFromForm(form);
			if (hook) await actions.update(hook.id, body);
			else oncreated?.(await actions.create(body));
			onclose();
		} catch (e) {
			failure = errMessage(e);
		} finally {
			busy = false;
		}
	}
</script>

<Modal open title={hook ? m.webhooks_edit_title() : m.webhooks_new_title()} {onclose} {busy} size="lg">
	{#snippet body()}
		<div class="editor" data-testid="webhook-editor">
			<Field
				label={m.webhooks_name_label()}
				for="{uid}-name"
				error={tried && problems.name ? m.webhooks_error_name() : undefined}
			>
				<Input id="{uid}-name" bind:value={form.name} placeholder={m.webhooks_name_placeholder()} />
			</Field>

			<Field
				label={m.webhooks_preset_label()}
				for="{uid}-preset"
				hint={m.webhooks_preset_hint()}
				error={tried && problems.preset ? m.webhooks_error_preset() : undefined}
			>
				<div class="preset">
					<div class="grow">
						<Select id="{uid}-preset" options={presetOptions} bind:value={form.preset_id} />
					</div>
					<Button size="sm" onclick={() => (newPreset = true)}>{m.webhooks_preset_new()}</Button>
				</div>
			</Field>

			<Field
				label={m.webhooks_template_label()}
				for="{uid}-template"
				hint={m.webhooks_template_hint({
					payloadExpr: '{{payload.issue.title}}',
					eventExpr: '{{event}}',
					deliveryExpr: '{{delivery}}'
				})}
				error={tried && problems.template ? m.webhooks_error_template() : undefined}
			>
				<Textarea id="{uid}-template" rows={4} block mono bind:value={form.prompt_template} />
			</Field>
			{#if expressions.unrecognised.length}
				<Text size="xs" tone="warn" data-testid="template-unrecognised">
					{m.webhooks_template_unrecognised({ list: expressions.unrecognised.map((e) => `{{${e}}}`).join(', ') })}
				</Text>
			{:else if expressions.recognised.length}
				<Text size="xs" tone="faint" data-testid="template-recognised">
					{m.webhooks_template_recognised({ list: expressions.recognised.map((e) => `{{${e}}}`).join(', ') })}
				</Text>
			{/if}

			<Field
				label={m.webhooks_rate_label()}
				for="{uid}-rate"
				hint={m.webhooks_rate_hint({ min: RATE_MIN, max: RATE_MAX })}
				error={tried && problems.rate ? m.webhooks_error_rate({ min: RATE_MIN, max: RATE_MAX }) : undefined}
			>
				<Input
					id="{uid}-rate"
					type="number"
					min={RATE_MIN}
					max={RATE_MAX}
					step={1}
					width="9rem"
					bind:value={() => form.rate_per_minute, (v) => (form.rate_per_minute = String(v ?? ''))}
				/>
			</Field>

			<Switch
				id="{uid}-enabled"
				bind:checked={form.enabled}
				label={m.webhooks_enabled_label()}
				labelVisible
				size="sm"
			/>

			{#if failure}
				<Callout tone="danger" icon="alert" role="alert">{failure}</Callout>
			{/if}
		</div>
	{/snippet}
	{#snippet footer()}
		<Button variant="ghost" disabled={busy} onclick={onclose}>{m.common_cancel()}</Button>
		<Button variant="primary" disabled={busy} onclick={save}>{m.common_save()}</Button>
	{/snippet}
</Modal>

{#if newPreset}
	<PresetEditorModal
		external
		title={m.webhooks_preset_new_title()}
		onclose={() => (newPreset = false)}
		onsaved={(saved) => {
			added = [...added, saved];
			form.preset_id = saved.id;
		}}
	/>
{/if}

<style>
	.editor {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
	}
	.preset {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
	}
	.grow {
		flex: 1;
		min-width: 0;
	}
</style>
