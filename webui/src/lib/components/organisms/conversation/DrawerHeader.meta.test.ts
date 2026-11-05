// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest';
import header from './HeaderMeta.svelte?raw';
import details from './HeaderMetaDetails.svelte?raw';
import model from './HeaderModel.svelte?raw';
import chip from '$lib/components/molecules/LangfuseChip.svelte?raw';
import host from './DrawerHeader.svelte?raw';
import en from '../../../../../messages/en.json?raw';
import fr from '../../../../../messages/fr.json?raw';

const markup = header.slice(header.indexOf('</script>'));
const script = header.slice(0, header.indexOf('</script>'));
const css = header.slice(header.indexOf('<style>'));
const modelCss = model.slice(model.indexOf('<style>'));
const trail = markup.slice(markup.indexOf('<div class="meta-trail">'), markup.indexOf('<Popover'));
const popover = markup.slice(markup.indexOf('<Popover'), markup.indexOf('</Popover>'));
const queryBody = (src: string, q: string) => {
	const at = src.indexOf(q);
	expect(at, q).toBeGreaterThan(-1);
	const rest = src.slice(at);
	return rest.slice(0, rest.indexOf('\n\t}'));
};

describe('drawer header meta row', () => {
	it('keeps cwd and branch on the row at every width', () => {
		expect(markup).toContain('<WorkingDir');
		expect(markup).toContain('m.sessions_branch_title({ branch })');
		for (const s of ['<WorkingDir', 'class="branch"']) expect(popover, s).not.toContain(s);
	});

	it('decides every width from the drawer container, never the viewport or JS', () => {
		expect(host).toContain('container: drawer-head / inline-size');
		expect(css).toContain('@container drawer-head (max-width: 40rem)');
		expect(css).toContain('@container drawer-head (max-width: 26rem)');
		for (const src of [css, modelCss]) expect(src).not.toContain('@media');
		expect(script).not.toContain('getComputedStyle');
	});

	it('drops the cost below 40rem, and the model and the token total below 26rem', () => {
		expect(queryBody(css, '@container drawer-head (max-width: 40rem)')).toContain('.cost');
		expect(queryBody(css, '@container drawer-head (max-width: 26rem)')).toContain('.tokens');
		const narrowModel = queryBody(modelCss, '@container drawer-head (max-width: 26rem)');
		expect(narrowModel).toContain('.strip .model,');
		expect(narrowModel).toContain('.strip .model-edit');
	});

	it('renders the branch name in the mono font, not the kit Badge mono flag', () => {
		const branch = markup.slice(markup.indexOf('{#if branch}'), markup.indexOf('{/if}', markup.indexOf('{#if branch}')));
		expect(branch).not.toMatch(/<Badge[^>]*\bmono\b/);
		expect(branch).toContain('<span class="ellipsis code">{branch}</span>');
		expect(css).toMatch(/\.code \{\n\t\tfont-family: var\(--font-mono\);/);
	});

	it('spells the token total as words, not Σ ↑ ↓ ⚡', () => {
		expect(trail).toContain('<StatText value={totals.total} kind="tokens" />');
		expect(header).not.toContain('<TokenUsage');
		for (const glyph of ['Σ', '↑', '↓', '⚡', '💥']) {
			expect(markup, glyph).not.toContain(glyph);
			expect(details.slice(details.indexOf('</script>')), glyph).not.toContain(glyph);
		}
	});

	it('shows one cost and names its source', () => {
		expect(script).toContain('headerCost(Number(session.token_usage.cost_usd) || 0, lf.data)');
		expect(trail).toContain("{#if cost.source === 'langfuse'}");
		expect(trail).toContain('<LangfuseChip id={session.id} />');
		expect(trail).toContain('m.chrome_cost_estimate(');
		expect(chip).toContain('m.chrome_cost_langfuse({ cost: costLabel })');
		expect(chip).not.toContain('sessions_langfuse_calls_short');
	});

	it('shows the cost once in the details, with its source and Langfuse link', () => {
		const cost = details.slice(details.indexOf('data-testid="header-cost"'));
		const cell = cost.slice(0, cost.indexOf('</span>'));
		expect(cell).toContain("{#if cost?.source === 'langfuse'}");
		expect(cell).toContain('<LangfuseChip id={session.id} />');
		expect(details.match(/<LangfuseChip/g)).toHaveLength(1);
		expect(details).not.toContain('m.drawer_details_langfuse()');
	});

	it('keeps row labels on one line next to a long value', () => {
		const dcss = details.slice(details.indexOf('<style>'));
		expect(dcss).toMatch(/\.mp-row > \.mp-key \{[^}]*flex: none;[^}]*white-space: nowrap;/);
		expect(dcss).toMatch(/\.mp-val \{[^}]*flex: 1 1 auto;/);
	});

	it('never paints a cost in the success colour', () => {
		for (const src of [header, details, chip]) expect(src).not.toContain('tone="success"');
	});

	it('labels a plan-billed session instead of showing $0.00', () => {
		const cost = details.slice(details.indexOf('data-testid="header-cost"'));
		expect(cost.slice(0, cost.indexOf('</span>'))).toContain('m.chrome_cost_plan()');
		expect(details).toContain('showHarnessEstimate(cost, harnessUsd)');
	});

	it('reads numbers in sans tabular figures, not mono', () => {
		const grid = details.slice(details.indexOf('<div class="mp-grid">'), details.indexOf('{#if bust}'));
		expect(grid).not.toContain('variant="code"');
		expect(grid).toContain('<Text numeric');
	});

	it('makes the adapter logo the one trigger', () => {
		expect(popover).toContain('{#snippet trigger()}<AdapterIcon');
		expect(trail).not.toContain('<Icon name="info"');
		expect(popover).toContain('box="sm"');
	});

	it('puts the full readout behind the trigger, model editor included', () => {
		expect(popover).toContain('<HeaderMetaDetails');
		for (const k of [
			'm.drawer_details_in()',
			'm.drawer_details_out()',
			'm.drawer_details_cache_read()',
			'm.drawer_details_cache_write()',
			'm.drawer_details_total()',
			'm.drawer_details_cost()',
			'<LangfuseChip'
		])
			expect(details, k).toContain(k);
		expect(details).toContain('idPrefix="drawer-details" inPopover');
		expect(model).toContain('<ModelPicker');
		expect(model).toContain('onclick={apply}');
		expect(model).toContain('onclick={() => (editing = false)}');
		expect(model).toContain('onclick={openEditor}');
	});

	it('gives the two model editor instances distinct ids', () => {
		expect(model).toContain('id="{idPrefix}-model"');
		expect(trail).toContain('idPrefix="drawer"');
		expect(model).not.toContain('id="drawer-model"');
	});

	it('names the trigger and lets the kit own aria-expanded and Escape', () => {
		expect(popover).toContain('label={m.drawer_meta_details()}');
		for (const msgs of [en, fr]) expect(JSON.parse(msgs).drawer_meta_details).toBeTruthy();
	});

	it('keeps the full model form inside the popover whatever the row width', () => {
		expect(model).toContain('class:strip={!inPopover}');
		const wide = queryBody(modelCss, '@container drawer-head (max-width: 40rem)');
		expect(wide).toContain('.strip .m-full');
		expect(wide).not.toMatch(/^\s+\.m-full/m);
	});

	it('clamps the panel to the viewport so it cannot clip on a phone', () => {
		expect(popover).toMatch(/panelStyle="[^"]*100vw - 2 \* var\(--sp-3\)[^"]*"/);
	});

	it('only shows the Langfuse rows when the sink reported calls', () => {
		expect(details).toContain('{#if calls > 0}');
		expect(script).toContain('() => langfuseAvailable');
	});

	it('adds no horizontal overflow escape hatch', () => {
		for (const src of [header, details, model]) {
			expect(src).not.toContain(':global(');
			expect(src).not.toContain('overflow-x');
		}
	});
});
