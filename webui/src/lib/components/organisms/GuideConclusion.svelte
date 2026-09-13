<script lang="ts">
	import { Badge, Button, Heading, Modal, Stack, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	let {
		title,
		xp,
		ondone
	}: {
		title: string;
		xp: number;
		ondone: () => void;
	} = $props();
</script>

{#snippet body()}
	<div class="beat">
		<Stack gap="md" align="center">
			<div class="seal" aria-hidden="true">◆</div>
			<Heading level={2} size="lg">{m.guide_done_title()}</Heading>
			<Text tone="muted">{m.guide_done_body({ title })}</Text>
			<div class="award">
				<Badge tone="ok" size="md" border>{m.guide_done_xp({ xp })}</Badge>
			</div>
		</Stack>
	</div>
{/snippet}

{#snippet footer()}
	<Button variant="primary" onclick={ondone}>{m.guide_done_continue()}</Button>
{/snippet}

<Modal title={m.guide_done_title()} size="sm" onclose={ondone} {body} {footer} />

<style>
	.beat {
		padding-block: var(--space-md);
		text-align: center;
	}

	.seal {
		color: var(--ok);
		font-size: 2.5rem;
		line-height: 1;
		animation: seal 520ms cubic-bezier(0.2, 1.4, 0.3, 1) both;
	}

	.award {
		animation: award 420ms ease-out 260ms both;
	}

	@keyframes seal {
		from {
			opacity: 0;
			transform: scale(0.4) rotate(-25deg);
		}
		to {
			opacity: 1;
			transform: scale(1) rotate(0deg);
		}
	}

	@keyframes award {
		from {
			opacity: 0;
			transform: translateY(0.5rem);
		}
		to {
			opacity: 1;
			transform: none;
		}
	}

	@media (prefers-reduced-motion: reduce) {
		.seal,
		.award {
			animation: none;
		}
	}
</style>
