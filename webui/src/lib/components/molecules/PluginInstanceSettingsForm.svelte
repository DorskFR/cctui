<script lang="ts">
	import { Badge, Button, Callout, EmptyState, Field, Input, Text } from '@dorsk/tsumikit';
	import { useQueryClient } from '@tanstack/svelte-query';
	import { copyText } from '$lib/clipboard';
	import { endpoints, qk, usePluginInstanceSettings } from '$lib/queries';
	import { toasts } from '$lib/toast.svelte';
	import { m } from '$lib/paraglide/messages';

	let { pluginId }: { pluginId: string } = $props();

	const settings = usePluginInstanceSettings(() => pluginId);
	const qc = useQueryClient();
	const decls = $derived(settings.data?.instance_settings ?? []);
	const upstreamKey = $derived(settings.data?.backend_upstream_setting ?? null);

	// Only what the admin typed is sent: an untouched key is omitted so the
	// server keeps its value, and a secret is never echoed back to seed a field.
	let draft = $state<Record<string, string>>({});
	let busy = $state(false);
	let revealed = $state<string | null>(null);

	const dirty = $derived(Object.keys(draft).length > 0);

	function valueOf(key: string): string {
		return draft[key] ?? settings.data?.values[key] ?? '';
	}

	function secretState(key: string): boolean {
		return settings.data?.secrets_set[key] === true;
	}

	async function run(work: () => Promise<unknown>, done: string) {
		busy = true;
		try {
			await work();
			await qc.invalidateQueries({ queryKey: qk.adminPluginSettings(pluginId) });
			await qc.invalidateQueries({ queryKey: qk.adminPlugins });
			await qc.invalidateQueries({ queryKey: qk.plugins });
			toasts.ok(done);
		} catch (e) {
			toasts.error(e instanceof Error ? e.message : String(e));
		} finally {
			busy = false;
		}
	}

	function save() {
		if (!dirty) return;
		const values = { ...draft };
		void run(async () => {
			await endpoints.savePluginInstanceSettings(pluginId, values);
			draft = {};
		}, m.settings_plugins_instance_saved());
	}

	function clearSecret(key: string) {
		if (!confirm(m.settings_plugins_instance_secret_clear_confirm())) return;
		void run(
			() => endpoints.savePluginInstanceSettings(pluginId, { [key]: '' }),
			m.settings_plugins_instance_secret_cleared()
		);
	}

	function rotate() {
		if (!confirm(m.settings_plugins_instance_proxy_rotate_confirm())) return;
		void run(async () => {
			const minted = await endpoints.rotatePluginProxySecret(pluginId);
			revealed = minted.secret;
		}, m.settings_plugins_instance_proxy_rotated());
	}
</script>

<div class="form" data-journey="plugin-instance-settings" data-plugin={pluginId}>
	{#if settings.isError}
		<EmptyState size="compact" tone="danger" icon="warning" title={m.settings_plugins_instance_failed()} />
	{:else if settings.isPending}
		<EmptyState size="compact" loading title={m.common_loading()} />
	{:else}
		{#if decls.length === 0}
			<Text size="sm" tone="faint" data-journey="plugin-instance-empty">{m.settings_plugins_instance_empty()}</Text>
		{/if}
		{#each decls as d (d.key)}
			<Field
				label={d.label}
				hint={d.key === upstreamKey ? m.settings_plugins_instance_upstream_hint() : undefined}
			>
				{#if d.secret}
					<div class="secret">
						<Input
							grow
							type="password"
							value={draft[d.key] ?? ''}
							autocomplete="off"
							spellcheck={false}
							placeholder={m.settings_plugins_instance_secret_placeholder()}
							data-journey="plugin-instance-secret"
							data-key={d.key}
							oninput={(e) => (draft = { ...draft, [d.key]: (e.currentTarget as HTMLInputElement).value })}
						/>
						<Badge
							size="sm"
							tone={secretState(d.key) ? 'success' : 'neutral'}
							data-journey="plugin-instance-secret-state"
							data-key={d.key}
						>
							{secretState(d.key)
								? m.settings_plugins_instance_secret_set()
								: m.settings_plugins_instance_secret_unset()}
						</Badge>
						{#if secretState(d.key)}
							<Button
								size="sm"
								variant="ghost"
								disabled={busy}
								data-journey="plugin-instance-secret-clear"
								data-key={d.key}
								onclick={() => clearSecret(d.key)}
							>
								{m.settings_plugins_instance_secret_clear()}
							</Button>
						{/if}
					</div>
				{:else}
					<Input
						value={valueOf(d.key)}
						autocomplete="off"
						spellcheck={false}
						inputmode={d.type === 'url' ? 'url' : undefined}
						data-journey="plugin-instance-field"
						data-key={d.key}
						oninput={(e) => (draft = { ...draft, [d.key]: (e.currentTarget as HTMLInputElement).value })}
					/>
				{/if}
			</Field>
		{/each}
		{#if decls.length > 0}
			<div class="actions">
				<Button size="sm" disabled={!dirty || busy} data-journey="plugin-instance-save" onclick={save}>
					{m.common_save()}
				</Button>
				{#if dirty}
					<Button size="sm" variant="ghost" disabled={busy} onclick={() => (draft = {})}>
						{m.common_cancel()}
					</Button>
				{/if}
			</div>
			<Text size="xs" tone="faint">{m.settings_plugins_instance_secret_help()}</Text>
		{/if}
		{#if upstreamKey !== null}
			<div class="proxy" data-journey="plugin-instance-proxy">
				<Text size="sm" weight="medium">{m.settings_plugins_instance_proxy_label()}</Text>
				<Badge size="sm" tone={settings.data?.proxy_secret_set ? 'success' : 'neutral'} data-journey="plugin-instance-proxy-state">
					{settings.data?.proxy_secret_set
						? m.settings_plugins_instance_secret_set()
						: m.settings_plugins_instance_secret_unset()}
				</Badge>
				<Button size="sm" variant="ghost" disabled={busy} data-journey="plugin-instance-proxy-rotate" onclick={rotate}>
					{m.settings_plugins_instance_proxy_rotate()}
				</Button>
			</div>
			<Text size="xs" tone="faint">{m.settings_plugins_instance_proxy_help()}</Text>
		{/if}
		{#if revealed}
			<Callout tone="warn" data-journey="plugin-instance-proxy-secret">
				<Text size="sm">{m.settings_plugins_instance_proxy_once()}</Text>
				<Text size="sm" variant="code" data-journey="plugin-instance-proxy-secret-value">{revealed}</Text>
				<div class="actions">
					<Button
						size="sm"
						variant="ghost"
						onclick={() => void copyText(revealed ?? '', m.settings_plugins_instance_proxy_copied())}
					>
						{m.common_copy()}
					</Button>
					<Button size="sm" variant="ghost" onclick={() => (revealed = null)}>{m.common_close()}</Button>
				</div>
			</Callout>
		{/if}
	{/if}
</div>

<style>
	.form {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		padding: 0 var(--sp-3) var(--sp-3);
		min-width: 0;
	}
	.secret,
	.actions,
	.proxy {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		flex-wrap: wrap;
		min-width: 0;
	}
</style>
