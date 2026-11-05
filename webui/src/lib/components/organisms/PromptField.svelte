<script lang="ts">
	import type { ComponentProps } from 'svelte';
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
	} = $props();

	const sessionsQuery = useSessions(() => false);
	const sessions = $derived(sessionsQuery.data?.sessions ?? []);
	const masking = $derived(redactedSpans(value).length > 0);
</script>

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
