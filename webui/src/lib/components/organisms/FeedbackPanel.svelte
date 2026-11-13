<script lang="ts">
	import { feedbackConfig } from '$lib/config';
	import { feedbackContext, feedbackLabels } from '$lib/feedback';

	let {
		serverVersion,
		route
	}: { serverVersion: () => string | undefined; route: () => string | null } = $props();

	const config = feedbackConfig();
	const hmm = config
		? import('@dorsk/hmm').catch((error) => {
				console.warn('feedback panel disabled:', error);
				return null;
			})
		: null;
	const labels = feedbackLabels();
</script>

{#if config && hmm}
	{#await hmm then mod}
		{#if mod}
			<div class="feedback">
				<mod.Hmm
					endpoint={config.endpoint}
					projectKey={config.projectKey}
					launcher={{ form: 'edge', side: 'right', align: 'end' }}
					{labels}
					context={() => feedbackContext(serverVersion(), route())}
				/>
			</div>
		{/if}
	{/await}
{/if}

<style>
	.feedback {
		display: contents;
		--hmm-offset-y: var(--bottom-chrome);
	}
</style>
