import { describe, expect, it } from 'vitest';
import header from './DrawerHeader.svelte?raw';
import meta from './HeaderMeta.svelte?raw';
import composer from './ConversationComposer.svelte?raw';
import pane from '../ConversationPane.svelte?raw';
import langfuseChip from '$lib/components/molecules/LangfuseChip.svelte?raw';
import profileForm from '../spawn/ProfileForm.svelte?raw';

const items = header.slice(
	header.indexOf('const overflowItems'),
	header.indexOf(']);', header.indexOf('const overflowItems'))
);
const prop = (src: string, tag: string, name: string) => {
	const markup = src.slice(src.indexOf('</script>'));
	const start = markup.indexOf(`<${tag}`);
	const body = markup.slice(start, markup.indexOf('/>', start));
	const at = body.indexOf(`${name}=`);
	return at < 0 ? '' : body.slice(at, body.indexOf('\n', at));
};

describe('drawer header gates', () => {
	it('offers keep-warm only with automation', () => {
		const at = items.indexOf('m.drawer_keepalive_label()');
		expect(items.lastIndexOf("hasFeature('automation')", at)).toBeGreaterThan(-1);
	});

	it('offers the room picker only with rooms', () => {
		expect(items).toContain("...(onsetroom && hasFeature('rooms')");
	});
});

describe('conversation pane gates', () => {
	it('wires find only with transcript search', () => {
		expect(prop(pane, 'DrawerHeader', 'onsearch')).toContain('searchable ?');
		expect(pane).toContain("const searchable = $derived(hasFeature('transcript_search'));");
		expect(pane).toContain('{#if search.open && searchable}');
	});

	it('wires follow-ups only with automation and machine spawn', () => {
		expect(pane).toMatch(/canFollowup = \$derived\(\s*!!onFollowup && hasFeature\('automation'\) && hasFeature\('machine_spawn'\)/);
		expect(prop(pane, 'DrawerHeader', 'onfollowup')).toContain('canFollowup ?');
		expect(prop(pane, 'ConversationComposer', 'onFollowup')).toContain('canFollowup ?');
	});

	it('wires the terminal only with terminal', () => {
		expect(pane).toContain("const canTerminal = $derived(harnessCaps.live_view && hasFeature('terminal'));");
		expect(pane).toContain('{#if terminalOpen && canTerminal}');
	});

	it('links local files only with files', () => {
		expect(pane).toContain("machineId: () => (hasFeature('files') ? session.machine_id : ''),");
	});

	it('takes attachments only with uploads', () => {
		expect(pane).toMatch(/supportsAttachments = \$derived\(\s*hasFeature\('uploads'\) &&/);
	});

	it('lists plugins only with plugins', () => {
		expect(pane).toContain("installed: () => (pluginsOn ? (installedPlugins.data ?? []) : []),");
	});

	it('sets rooms only with rooms', () => {
		expect(prop(pane, 'DrawerHeader', 'onsetroom')).toContain("hasFeature('rooms')");
	});
});

describe('header meta gates', () => {
	it('mounts the thread menu only with automation', () => {
		expect(meta).toMatch(/\{#if hasFeature\('automation'\)\}\s*<ThreadMenu/);
	});

	it('reads Langfuse only with observability', () => {
		expect(meta).toContain("hasFeature('observability') && !!caps.data?.langfuse?.available");
		expect(langfuseChip).toContain("hasFeature('observability') && !!lf?.available");
	});
});

describe('composer gates', () => {
	it('mounts the scheduled queue only with scheduling', () => {
		expect(composer).toMatch(/\{#if schedulingOn\}\s*<div class="scheduled"/);
	});

	it('drops the schedule presets without scheduling', () => {
		expect(composer).toContain('if (!schedulingOn) return coldItems ? coldSendItems(coldItems) : [];');
		expect(composer).toContain('menu={schedulingOn || coldItems !== null}');
	});

	it('refuses to schedule without scheduling', () => {
		const fn = composer.slice(composer.indexOf('async function scheduleAt('));
		expect(fn.slice(0, fn.indexOf('\n\t}\n'))).toContain('if (!schedulingOn ||');
	});
});

describe('profile editor gates', () => {
	it('pins context only with context', () => {
		expect(profileForm).toContain('useContextItems(() => contextOn)');
		expect(profileForm).toMatch(/\{#if contextOn\}\s*<Field label=\{m\.spawn_profile_context_label\(\)\}/);
	});
});
