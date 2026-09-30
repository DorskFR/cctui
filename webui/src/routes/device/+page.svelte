<script lang="ts">
	import type { DeviceAuthRequestInfo } from '@bindings/DeviceAuthRequestInfo';
	import { page } from '$app/state';
	import { Button, Callout, Cluster, Input, Spinner, Stack, Text } from '@dorsk/tsumikit';
	import PageHead from '$lib/components/molecules/PageHead.svelte';
	import {
		codeFromSearch,
		isCompleteUserCode,
		isDecidable,
		normalizeUserCode
	} from '$lib/deviceAuth';
	import { endpoints } from '$lib/queries/endpoints';
	import { errMessage } from '$lib/api';
	import { toasts } from '$lib/toast.svelte';
	import { m } from '$lib/paraglide/messages';

	let code = $state(codeFromSearch(page.url.searchParams));
	let info = $state<DeviceAuthRequestInfo | null>(null);
	let error = $state('');
	let busy = $state(false);
	let decided = $state<'approved' | 'denied' | null>(null);

	async function lookup() {
		const wanted = normalizeUserCode(code);
		if (!isCompleteUserCode(wanted)) return;
		busy = true;
		error = '';
		decided = null;
		try {
			info = await endpoints.deviceAuthInfo(wanted);
		} catch (e) {
			info = null;
			error = errMessage(e);
		} finally {
			busy = false;
		}
	}

	async function decide(approve: boolean) {
		if (!info) return;
		busy = true;
		try {
			await endpoints.deviceAuthDecide(info.user_code, approve);
			decided = approve ? 'approved' : 'denied';
			toasts.ok(approve ? m.device_approved() : m.device_denied());
		} catch (e) {
			error = errMessage(e);
		} finally {
			busy = false;
		}
	}

	// A code that arrived in the link is looked up straight away: the user came
	// here by clicking it, not to type it again. Once only — after that the
	// form is theirs.
	let autoLookedUp = false;
	$effect(() => {
		if (autoLookedUp) return;
		autoLookedUp = true;
		if (isCompleteUserCode(code)) void lookup();
	});
</script>

<PageHead title={m.device_title()} />

<Stack gap="var(--sp-4)">
	<Text tone="muted">{m.device_intro()}</Text>

	<form
		onsubmit={(e) => {
			e.preventDefault();
			void lookup();
		}}
	>
		<Cluster gap="var(--sp-2)" align="center">
			<Input
				aria-label={m.device_code_label()}
				placeholder="XXXX-XXXX"
				value={code}
				oninput={(e) =>
					(code = normalizeUserCode((e.currentTarget as HTMLInputElement).value))}
			/>
			<Button type="submit" disabled={busy || !isCompleteUserCode(code)}>
				{m.device_lookup()}
			</Button>
		</Cluster>
	</form>

	{#if busy && !info}
		<Spinner label={m.common_loading()} />
	{/if}

	{#if error}
		<Callout tone="danger">{error}</Callout>
	{/if}

	{#if decided}
		<Callout tone={decided === 'approved' ? 'success' : 'warn'}>
			{decided === 'approved' ? m.device_approved() : m.device_denied()}
		</Callout>
	{:else if info}
		<Stack gap="var(--sp-2)">
			<Text>{m.device_client({ name: info.client_name ?? m.device_unknown_client() })}</Text>
			<Text variant="code">{info.user_code}</Text>
			{#if isDecidable(info.status)}
				<Text size="sm" tone="muted">{m.device_expires_in({ seconds: info.expires_in_secs })}</Text>
				<Callout tone="warn">{m.device_warning()}</Callout>
				<Cluster gap="var(--sp-2)" align="center">
					<Button variant="primary" disabled={busy} onclick={() => decide(true)}>
						{m.device_approve()}
					</Button>
					<Button disabled={busy} onclick={() => decide(false)}>{m.device_deny()}</Button>
				</Cluster>
			{:else}
				<Callout tone="danger">{m.device_status_dead()}</Callout>
			{/if}
		</Stack>
	{/if}
</Stack>
