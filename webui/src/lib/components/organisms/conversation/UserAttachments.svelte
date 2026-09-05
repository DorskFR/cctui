<script lang="ts">
	// Chips under a user bubble for the files that message uploaded: a
	// `paste-N.txt` mask expands inline, an image shows as a thumbnail, anything
	// else opens through the remote-file viewer (overlay or download).
	import { Button, IconButton, Text } from '@dorsk/tsumikit';
	import { fmtSize } from '$lib/attachments';
	import { copyText } from '$lib/clipboard';
	import { openLocalFile } from '$lib/fileviewer';
	import { m } from '$lib/paraglide/messages';
	import { useSessionAttachments } from '$lib/queries';
	import { attachmentBlobUrl, pickAttachment, type SessionAttachment } from '$lib/queries/types';
	import { toasts } from '$lib/toast.svelte';
	import { isPasteName, type UserUploadRefs } from './lines';

	let { refs, ts }: { refs: UserUploadRefs; ts: number } = $props();

	const query = useSessionAttachments(
		() => refs.sessionId ?? '',
		() => !!refs.sessionId && refs.names.length > 0
	);

	const resolved = $derived(
		refs.names
			.map((name) => pickAttachment(query.data ?? [], name, ts))
			.filter((a): a is SessionAttachment => a !== null)
	);

	const url = (a: SessionAttachment) => attachmentBlobUrl(a.session_id, a.hash);
	const isImage = (a: SessionAttachment) => (a.content_type ?? '').startsWith('image/');
	const isPaste = (a: SessionAttachment) => isPasteName(a.name);

	let expanded = $state<Record<string, boolean>>({});
	let texts = $state<Record<string, string>>({});

	async function loadText(a: SessionAttachment): Promise<string | null> {
		if (texts[a.id] !== undefined) return texts[a.id];
		try {
			const res = await fetch(url(a), { credentials: 'same-origin' });
			if (!res.ok) throw new Error(String(res.status));
			const text = await res.text();
			texts[a.id] = text;
			return text;
		} catch {
			toasts.error(m.conversation_attachment_load_failed({ name: a.name }));
			return null;
		}
	}

	async function toggle(a: SessionAttachment) {
		if (!expanded[a.id] && (await loadText(a)) === null) return;
		expanded[a.id] = !expanded[a.id];
	}

	async function copy(a: SessionAttachment) {
		const text = await loadText(a);
		if (text !== null) await copyText(text);
	}

	const lineCount = (text: string) => text.split('\n').length;
</script>

{#if resolved.length}
	<div class="attachments">
		{#each resolved as a (a.id)}
			{#if isPaste(a)}
				<div class="paste">
					<div class="paste-head">
						<Button
							chip
							size="sm"
							variant="ghost"
							icon
							iconInline
							aria-expanded={!!expanded[a.id]}
							title={expanded[a.id]
								? m.conversation_attachment_collapse({ name: a.name })
								: m.conversation_attachment_expand({ name: a.name })}
							onclick={() => toggle(a)}
						>
							<span class="name">{a.name}</span>
							<Text size="xs" tone="faint" as="span">
								{texts[a.id] !== undefined
									? m.conversation_attachment_lines({ lines: lineCount(texts[a.id]) })
									: fmtSize(a.size)}
							</Text>
						</Button>
						<IconButton
							inline
							icon="copy"
							label={m.conversation_attachment_copy({ name: a.name })}
							title={m.conversation_attachment_copy({ name: a.name })}
							onclick={() => copy(a)}
						/>
					</div>
					{#if expanded[a.id] && texts[a.id] !== undefined}
						<pre class="paste-body mono">{texts[a.id]}</pre>
					{/if}
				</div>
			{:else if isImage(a)}
				<button
					type="button"
					class="thumb"
					title={m.conversation_attachment_open({ name: a.name })}
					onclick={() => openLocalFile(url(a), a.name)}
				>
					<img src={url(a)} alt={a.name} loading="lazy" />
				</button>
			{:else}
				<Button
					chip
					size="sm"
					variant="ghost"
					title={m.conversation_attachment_open({ name: a.name })}
					onclick={() => openLocalFile(url(a), a.name)}
				>
					<span class="name">{a.name}</span>
					<Text size="xs" tone="faint" as="span">{fmtSize(a.size)}</Text>
				</Button>
			{/if}
		{/each}
	</div>
{/if}

<style>
	.attachments {
		display: flex;
		flex-wrap: wrap;
		align-items: flex-start;
		gap: var(--sp-1) var(--sp-2);
		margin-top: 2px;
	}
	.paste {
		display: flex;
		flex-direction: column;
		min-width: 0;
		max-width: 100%;
	}
	.paste-head {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-1);
	}
	.name {
		font-family: var(--font-mono);
		font-size: var(--fs-xs);
		margin-right: var(--sp-1);
	}
	.paste-body {
		margin: var(--sp-1) 0 0;
		padding: var(--sp-2);
		max-height: 22rem;
		overflow: auto;
		white-space: pre-wrap;
		font-size: calc(var(--fs-sm) - 0.0625rem);
		background: var(--bg-elevated-2);
		border: 1px solid var(--border);
		border-radius: var(--r-sm);
	}
	.thumb {
		padding: 0;
		background: none;
		border: 1px solid var(--border);
		border-radius: var(--r-sm);
		cursor: zoom-in;
		overflow: hidden;
		line-height: 0;
	}
	.thumb img {
		display: block;
		max-width: 12rem;
		max-height: 8rem;
		object-fit: contain;
	}
</style>
