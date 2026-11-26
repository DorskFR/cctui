<script lang="ts">
	import { settings } from '#lib/settings.svelte.ts';
	import { ThemePicker, FontScalePicker } from '@dorsk/tsumikit';
	import MainNav from '#lib/components/organisms/MainNav.svelte';
	import UsageBattery from '#lib/components/molecules/UsageBattery.svelte';
	import ResourceBattery from '#lib/components/molecules/ResourceBattery.svelte';
	import HeaderBrand from './header/HeaderBrand.svelte';
	import NotificationBell from '#lib/components/organisms/NotificationBell.svelte';
	import AccountMenu from './header/AccountMenu.svelte';
	import HeaderStatus from './header/HeaderStatus.svelte';
	import { installShellWatchers } from './header/shellWatchers.svelte.ts';
	import { m } from '#lib/paraglide/messages.js';

	installShellWatchers();
</script>

<header class="hd">
	<div class="hd-inner">
		<HeaderBrand />
		<div class="tabs">
			{#if settings.nav === 'top'}
				<MainNav placement="top" />
			{/if}
		</div>
		<div class="tail">
			<span class="batt"
				><span class="wide"><ResourceBattery /><UsageBattery /></span
				><span class="narrow"><HeaderStatus /></span></span
			>
			<span class="prefs">
				<ThemePicker
					auto
					autoLabel={m.theme_auto_label()}
					autoHelp={m.theme_auto_help()}
					lightLabel={m.theme_group_light()}
					darkLabel={m.theme_group_dark()}
					box="md"
					placement="bottom-end"
				/>
				<FontScalePicker />
			</span>
			<NotificationBell />
			<AccountMenu />
		</div>
	</div>
</header>

<style>
	.hd {
		position: fixed;
		top: 0;
		left: 0;
		right: 0;
		z-index: var(--z-header);
		background: color-mix(in srgb, var(--bg-elevated) 92%, transparent);
		backdrop-filter: blur(8px);
		border-bottom: 1px solid var(--border);
		padding-top: var(--safe-top);

		/* The app scales by changing the ROOT font-size; the header is chrome
		   that must not reflow with it, so its size tokens are pinned in px
		   (the :root rem values at the 16px base). */
		--fs-xs: 12px;
		--fs-sm: 13px;
		--fs-base: 15px;
		--fs-lg: 18px;
		--sp-1: 4px;
		--sp-2: 8px;
		--sp-3: 12px;
		--sp-4: 16px;
		--box-xs: 24px;
		--box-sm: 30px;
		--box-md: 36px;
		--box-lg: 40px;
	}
	.hd-inner {
		width: 100%;
		padding-inline: max(var(--sp-4), var(--safe-left)) max(var(--sp-4), var(--safe-right));
		height: var(--header-h);
		/* Side tracks share what the nav leaves equally, so the nav sits on the
		   bar's centre; a side never shrinks below its content, which pushes the
		   nav off-centre rather than under the account cluster. */
		display: grid;
		grid-template-columns: minmax(max-content, 1fr) minmax(0, 576px) minmax(max-content, 1fr);
		align-items: center;
		gap: var(--sp-2);
	}
	.tail {
		display: flex;
		align-items: center;
		justify-content: flex-end;
		gap: var(--sp-2);
		min-width: 0;
	}
	.tabs {
		display: none;
		min-width: 0;
		align-self: stretch;
		justify-content: center;
	}
	@media (min-width: 48rem) {
		.tabs {
			display: flex;
		}
	}
	/* No room for the centre track: the brand keeps its width, the cluster takes the rest. */
	@media (max-width: 47.999rem) {
		.hd-inner {
			grid-template-columns: auto minmax(0, 1fr);
		}
	}
	.prefs {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-1);
		flex: none;
	}
	.batt {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		flex: none;
	}
	.wide {
		display: contents;
	}
	.narrow {
		display: none;
	}
	/* Below the nav breakpoint machines and usage fold into one 44px status button. */
	@media (max-width: 47.999rem) {
		.wide {
			display: none;
		}
		.narrow {
			display: contents;
		}
	}
</style>
