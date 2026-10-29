<script lang="ts">
	import { Card, Container, Heading, Link, Stack, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
</script>

<main class="core-notice">
	<Container>
		<Card>
			<Stack>
				<Heading level={1}>{m.core_server_title()}</Heading>
				<Text>{m.core_server_description()}</Text>
				<Text>{m.core_server_action()}</Text>
				<Link href="https://github.com/DorskFR/cctui/blob/0.27.0/docs/core-server.md">
					{m.core_server_guide()}
				</Link>
			</Stack>
		</Card>
	</Container>
</main>

<style>
	.core-notice {
		padding-block: var(--sp-6);
	}
</style>
