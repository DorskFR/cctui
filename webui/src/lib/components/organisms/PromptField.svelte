<script lang="ts">
	import type { ComponentProps, Snippet } from 'svelte';
	import { Textarea } from '@dorsk/tsumikit';
	import SessionMention from '$lib/components/molecules/SessionMention.svelte';
	import { useSessions } from '$lib/queries';
	import type { PromptAttachments } from '$lib/promptAttachments.svelte';
	import { m } from '$lib/paraglide/messages';
	import { redactedSegments, redactedSpans } from '$lib/redactTag';

	let {
		value = $bindable(''),
		el = $bindable(null),
		att,
		excludeId = null,
		placement = 'auto',
		placeholder,
		revealed,
		tools,
		...rest
	}: Omit<ComponentProps<typeof Textarea>, 'value' | 'el' | 'onpaste'> & {
		value: string;
		el?: HTMLTextAreaElement | null;
		/** Omitted where the prompt takes no files. */
		att?: PromptAttachments;
		excludeId?: string | null;
		placement?: 'up' | 'auto';
		/** Values of closed `<redacted>` spans shown in clear. */
		revealed?: ReadonlySet<string>;
		/** Controls in the field's own bottom toolbar (schedule, mic…). */
		tools?: Snippet;
	} = $props();

	const sessionsQuery = useSessions(() => false);
	const sessions = $derived(sessionsQuery.data?.sessions ?? []);
	const masking = $derived(redactedSpans(value).length > 0);
</script>

{#snippet field()}
	<SessionMention bind:value {el} {sessions} {excludeId} {placement}>
		<Textarea
			{...rest}
			bind:value
			bind:el
			placeholder={att?.dragActive ? m.composer_drop_files() : placeholder}
			onpaste={att?.onPaste}
			segments={masking ? (v: string) => redactedSegments(v, revealed) : undefined}
		/>
	</SessionMention>
{/snippet}

{#if tools}
	<div class="prompt-field">
		{@render field()}
		<div class="tools" role="toolbar" aria-label={m.tx_prompt_tools()}>{@render tools()}</div>
	</div>
{:else}
	{@render field()}
{/if}

<style>
	.prompt-field {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
		min-width: 0;
	}
	.tools {
		display: flex;
		align-items: center;
		justify-content: flex-end;
		gap: var(--sp-1);
		flex-wrap: wrap;
	}
</style>
