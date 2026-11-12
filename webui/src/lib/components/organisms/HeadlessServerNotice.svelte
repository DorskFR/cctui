<script lang="ts">
	import { Card, Container, Heading, Stack, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
</script>

<main class="headless-notice">
	<Container>
		<Card>
			<Stack>
				<Heading level={1}>{m.headless_server_title()}</Heading>
				<Text>{m.headless_server_description()}</Text>
			</Stack>
		</Card>
	</Container>
</main>

<style>
	.headless-notice {
		padding-block: var(--sp-6);
	}
</style>
