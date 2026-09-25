import { describe, expect, it } from 'vitest';
import header from './DrawerHeader.svelte?raw';
import en from '../../../../../messages/en.json?raw';
import fr from '../../../../../messages/fr.json?raw';

const markup = header.slice(header.indexOf('</script>'));
const script = header.slice(0, header.indexOf('</script>'));
const trail = markup.slice(markup.indexOf('<div class="meta-trail">'), markup.indexOf('{#if keepaliveOpen}'));
const popover = markup.slice(markup.indexOf('<Popover'), markup.indexOf('</Popover>'));

describe('drawer header meta row', () => {
	it('keeps cwd and branch on the row at every width', () => {
		expect(markup).toContain('<WorkingDir');
		expect(markup).toContain('m.sessions_branch_title({ branch })');
		for (const s of ['<WorkingDir', 'class="branch"']) expect(popover, s).not.toContain(s);
	});

	it('drops langfuse below 40rem and the model badge below 26rem', () => {
		expect(script).toContain('const LANGFUSE_BELOW_REM = 40;');
		expect(script).toContain('const MODEL_BELOW_REM = 26;');
		expect(script).toContain('headWidth < LANGFUSE_BELOW_REM * rootFontPx');
		expect(script).toContain('headWidth < MODEL_BELOW_REM * rootFontPx');
		expect(markup).toContain('bind:clientWidth={headWidth}');
		expect(trail).toContain('{#if !hideLangfuse}');
		expect(trail).toContain("{#if !hideModel}{@render modelMeta('drawer')}{/if}");
	});

	it('measures the width in root font units rather than assuming 16px', () => {
		expect(script).toContain('getComputedStyle(document.documentElement).fontSize');
	});

	it('shows the ⓘ trigger only once something is hidden', () => {
		expect(script).toMatch(/showDetails = \$derived\(\(hideLangfuse \|\| hideModel\) && hasModelMeta\)/);
		expect(trail).toContain('{#if showDetails}');
		expect(script).toContain('let headWidth = $state(Infinity);');
	});

	it('puts the hidden items behind the trigger, model editor included', () => {
		expect(popover).toContain('{#if hideLangfuse}<span class="langfuse"><LangfuseChip');
		expect(popover).toContain("{@render modelMeta('drawer-details')}");
		const snippet = markup.slice(markup.indexOf('{#snippet modelMeta'), markup.indexOf('{#snippet modelMeta') + 2000);
		expect(snippet).toContain('<ModelPicker');
		expect(snippet).toContain('onclick={applyModelChange}');
		expect(snippet).toContain('onclick={() => (modelEditing = false)}');
		expect(snippet).toContain('onclick={openModelEditor}');
	});

	it('gives the two model editor instances distinct ids', () => {
		expect(markup).toContain('{#snippet modelMeta(idPrefix: string)}');
		expect(markup).toContain('id="{idPrefix}-model"');
		expect(markup).not.toContain('id="drawer-model"');
	});

	it('names the trigger and lets the kit own aria-expanded and Escape', () => {
		expect(popover).toContain('label={m.drawer_meta_details()}');
		expect(popover).toContain("{#snippet trigger()}<Icon name=\"info\"");
		expect(header).toContain('Popover,');
		for (const msgs of [en, fr]) expect(JSON.parse(msgs).drawer_meta_details).toBeTruthy();
	});

	it('restores the full model text inside the popover', () => {
		const css = header.slice(header.indexOf('<style>'));
		expect(css).toContain('.metapop .m-full');
		expect(css).toContain('.metapop .m-short');
		expect(css).not.toMatch(/@container drawer-head \(max-width: 26rem\)/);
		expect(css).not.toMatch(/\.langfuse,\n\s*\.m-effort/);
	});

	it('adds no horizontal overflow escape hatch', () => {
		expect(header).not.toContain(':global(');
		expect(header).not.toContain('overflow-x');
	});
});
