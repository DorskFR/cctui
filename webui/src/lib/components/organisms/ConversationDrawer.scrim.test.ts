import { describe, expect, it } from 'vitest';
import drawer from './ConversationDrawer.svelte?raw';

const markup = drawer.slice(drawer.indexOf('</script>'));

describe('conversation drawer scrim', () => {
	it('dims the page at 40% instead of the kit scrim', () => {
		expect(markup).toContain('scrim={false}');
		const scrim = markup.slice(markup.indexOf('<Scrim'), markup.indexOf('/>', markup.indexOf('<Scrim')));
		expect(scrim).toContain('style="background: rgb(0 0 0 / 40%)"');
		expect(scrim).toContain('{onclose}');
	});

	it('drops the scrim where the drawer goes full width', () => {
		const scrim = markup.slice(markup.indexOf('<Scrim'), markup.indexOf('/>', markup.indexOf('<Scrim')));
		expect(scrim).toContain('hideBelow="959px"');
		expect(markup).toContain('fullWidthBelow="959px"');
	});

	it('paints the scrim under the panel', () => {
		expect(markup.indexOf('<Scrim')).toBeLessThan(markup.indexOf('<ResizablePanel'));
	});
});
