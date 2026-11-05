import { describe, expect, it } from 'vitest';
import { MIN_PANE_CHROME, MIN_PANE_HEIGHT } from '$lib/tiles';
import header from './DrawerHeader.svelte?raw';
import menu from './drawerMenu.ts?raw';
import composer from './ConversationComposer.svelte?raw';
import pane from '../ConversationPane.svelte?raw';

const markup = header.slice(header.indexOf('</script>'));

describe('a tile wears one row of header chrome', () => {
	it('is the pane that asks for it, drawer untouched', () => {
		expect(pane).toContain("compact={chrome === 'tile'}");
		expect(header).toContain('compact = false');
		expect(composer).toContain('compact = false');
	});

	it('drops the meta row and the label strip', () => {
		expect(markup).toMatch(/\{#if !compact\}\s*<HeaderMeta/);
		expect(markup).toContain('{#if session.labels.length > 0 && !compact}');
	});

	it('keeps archive behind the ⋯ menu and Stop only while a turn runs', () => {
		expect(menu).toContain("if (!s.archived) session.push({ label: m.drawer_archive(), icon: 'archive'");
		expect(markup).not.toMatch(/<IconButton[^>]*icon="archive"/);
		expect(markup).toContain('{#if stoppable}');
	});

	it('keeps the bar in its collapsed density whatever a tile measures', () => {
		expect(header).toContain('$derived(compact || barWidth < COLLAPSE_BELOW)');
	});

	it('drops the Assistant/User/Tools filter bar', () => {
		expect(pane).toContain("{#if chrome !== 'tile'}");
		const gated = pane.slice(pane.indexOf("{#if chrome !== 'tile'}"));
		expect(gated.slice(0, gated.indexOf('{/if}'))).toContain('<DrawerToolbar');
	});
});

describe('a tile composer is one line until focused', () => {
	it('folds only in a tile, and unfolds on focus', () => {
		expect(composer).toContain('const folded = $derived(compact && !focused)');
		expect(composer).toContain('onfocusin={() => (focused = true)}');
		expect(composer).toContain('onfocusout={() => (focused = false)}');
	});

	it('hides the extra rows while folded, keeping the input and send', () => {
		expect(composer).toContain('class:hidden={folded}');
		expect(composer).toContain('{#if coldOffer && !folded}');
		expect(composer).toContain('leading={(supportsAttachments || voiceEnabled) && !folded ? leadingTools : undefined}');
		expect(composer).toContain('rows={1}');
	});
});

describe('the pane floor follows the lighter chrome', () => {
	it('is the one-row chrome plus a readable transcript', () => {
		expect(MIN_PANE_CHROME).toBeLessThanOrEqual(160);
		expect(MIN_PANE_HEIGHT).toBeLessThan(260);
	});
});
