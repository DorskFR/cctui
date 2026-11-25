<script lang="ts">
	import { settings } from '#lib/settings.svelte.ts';
	import { ThemePicker, FontScalePicker } from '@dorsk/tsumikit';
	import MainNav from '#lib/components/organisms/MainNav.svelte';
	import UsageBattery from '#lib/components/molecules/UsageBattery.svelte';
	import ResourceBattery from '#lib/components/molecules/ResourceBattery.svelte';
	import ProviderStatusDot from '#lib/components/molecules/ProviderStatusDot.svelte';
	import NotificationBell from '#lib/components/organisms/NotificationBell.svelte';
	import HeaderBrand from './header/HeaderBrand.svelte';
	import AccountMenu from './header/AccountMenu.svelte';
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
			<span class="batt"><ProviderStatusDot /><ResourceBattery /><UsageBattery /></span>
			<span class="divider" aria-hidden="true"></span>
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
		/* The brand and the account cluster take the width they need; the nav
		   centres in what is left. Equal thirds put the nav on the header's true
		   centre but capped the cluster at a third, which the usage strip
		   outgrows — it then spilled back over the nav. */
		display: grid;
		grid-template-columns: auto minmax(0, 1fr) auto;
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
	.batt:empty {
		display: none;
	}
	.divider {
		flex: none;
		width: 1px;
		height: 20px;
		background: var(--border);
	}
	.batt:empty + .divider {
		display: none;
	}
</style>
