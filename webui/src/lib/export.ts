/**
 * Export a conversation as a single self-contained HTML document (CCT-227).
 *
 * Built entirely client-side from the merged event list the drawer already
 * holds — no server round-trip. The file embeds all CSS (dark theme on screen,
 * light palette + page setup under `@media print`) so the browser's
 * Print → "Save as PDF" yields a clean PDF: that IS the PDF path; we don't
 * ship a PDF generator.
 *
 * Unlike the drawer's `lines` (gated by the view toggles), the export always
 * includes everything — tools, MCP, system, results — it's the archival copy.
 */

import type { AgentEvent } from '@bindings/AgentEvent';
import type { SessionListItem } from '@bindings/SessionListItem';
import { renderMarkdown, highlightBlock, prettyJson, escapeHtml } from '$lib/markdown';
import { USER_PREFIX } from '$lib/ws.svelte';

interface Block {
	role: 'assistant' | 'user' | 'system' | 'tool' | 'result' | 'reset' | 'compact' | 'ask';
	ts: number;
	label?: string; // tool name / divider text
	html: string; // inner HTML, already escaped/rendered
}

const META_TAGS = ['<task-notification', '<system-reminder', '<command-name', '<command-message', '<local-command', '<bash-input', '<bash-stdout', '<bash-stderr'];
const looksMeta = (t: string) => META_TAGS.some((m) => t.trimStart().startsWith(m));

// Mirror the drawer's tool-input prettification (diff / shell / JSON) so the
// export reads like the live view, without depending on its view state.
function formatToolInput(tool: string, input: unknown): string {
	const obj = input as Record<string, unknown> | null;
	if (obj && typeof obj === 'object' && 'old_string' in obj && 'new_string' in obj) {
		const minus = String(obj.old_string ?? '').split('\n').map((l) => `- ${l}`).join('\n');
		const plus = String(obj.new_string ?? '').split('\n').map((l) => `+ ${l}`).join('\n');
		return highlightBlock(`${obj.file_path ?? ''}\n${minus}\n${plus}`.trim(), '');
	}
	if (obj && typeof obj === 'object' && typeof obj.command === 'string') {
		const desc = typeof obj.description === 'string' && obj.description.trim() ? `# ${obj.description.trim()}\n` : '';
		return highlightBlock(`${desc}${obj.command}`, 'sh');
	}
	return highlightBlock(prettyJson(input).replace(/\\n/g, '\n').replace(/\\t/g, '\t'), 'json');
}

// AskUserQuestion inputs render as a readable Q/options card, not raw JSON.
function formatAsk(input: unknown): string | null {
	const qs = (input as { questions?: unknown })?.questions;
	if (!Array.isArray(qs) || qs.length === 0) return null;
	const parts: string[] = [];
	for (const q of qs as { question?: string; options?: { label?: string; description?: string }[] }[]) {
		if (typeof q?.question !== 'string') continue;
		const opts = (q.options ?? [])
			.map((o) => `<li><strong>${escapeHtml(String(o.label ?? ''))}</strong>${o.description ? ` — ${escapeHtml(o.description)}` : ''}</li>`)
			.join('');
		parts.push(`<p class="ask-q">${escapeHtml(q.question)}</p><ul class="ask-opts">${opts}</ul>`);
	}
	return parts.length ? parts.join('') : null;
}

function toBlock(e: AgentEvent): Block | null {
	switch (e.type) {
		case 'text': {
			if (!e.content.trim()) return null;
			if (e.content.startsWith(USER_PREFIX)) {
				const content = e.content.slice(USER_PREFIX.length).trimStart();
				const role = e.meta || looksMeta(content) ? 'system' : 'user';
				return { role, ts: Number(e.ts), html: renderMarkdown(content) };
			}
			return { role: 'assistant', ts: Number(e.ts), html: renderMarkdown(e.content) };
		}
		case 'reply':
			if (!e.content.trim()) return null;
			return { role: 'user', ts: Number(e.ts), html: renderMarkdown(e.content) };
		case 'tool_call': {
			if (e.tool === 'AskUserQuestion') {
				const ask = formatAsk(e.input);
				if (ask) return { role: 'ask', ts: Number(e.ts), label: 'AskUserQuestion', html: ask };
			}
			return { role: 'tool', ts: Number(e.ts), label: e.tool, html: `<pre><code>${formatToolInput(e.tool, e.input)}</code></pre>` };
		}
		case 'tool_result':
			return { role: 'result', ts: Number(e.ts), label: e.tool, html: `<pre><code>${highlightBlock(e.output_summary, '')}</code></pre>` };
		case 'context_reset':
			return { role: 'reset', ts: Number(e.ts), html: '⟳ context reset · /clear or /compact' };
		case 'compact_summary':
			if (!e.content.trim()) return null;
			return { role: 'compact', ts: Number(e.ts), html: renderMarkdown(e.content) };
		default:
			return null; // heartbeat, turn_end
	}
}

const ROLE_LABEL: Record<Block['role'], string> = {
	assistant: 'Assistant',
	user: 'User',
	system: 'System',
	tool: 'Tool',
	result: 'Result',
	ask: 'Question',
	reset: '',
	compact: 'Compacted context'
};

function fmtTs(ts: number): string {
	const d = new Date(ts);
	return isNaN(d.getTime()) ? '' : d.toLocaleString();
}

// One fixed palette (matches the app's dark theme feel) + a light print
// palette. Kept literal — the export must not depend on the app's stylesheets.
const CSS = `
:root{--bg:#101418;--panel:#171c22;--border:#2a3138;--text:#e6e9ec;--muted:#aab2ba;--faint:#6b747d;
--blue:#6cb6ff;--green:#7ee08a;--amber:#f0c674;--violet:#c89bf0;--red:#ff8a8a;
--md-text:var(--muted);--md-strong:var(--text);--md-code:var(--blue);--md-code-bg:rgba(108,182,255,.12);--md-heading:var(--text);
--syn-keyword:var(--violet);--syn-string:var(--green);--syn-number:var(--amber);--syn-comment:var(--faint);--syn-function:var(--blue)}
*{box-sizing:border-box}
body{margin:0;background:var(--bg);color:var(--text);font:14px/1.5 ui-monospace,SFMono-Regular,Menlo,Consolas,monospace}
.page{max-width:900px;margin:0 auto;padding:24px 20px 48px}
header{border-bottom:1px solid var(--border);padding-bottom:14px;margin-bottom:18px}
header h1{font-size:18px;margin:0 0 8px;word-break:break-word}
.meta{display:flex;flex-wrap:wrap;gap:6px 14px;color:var(--muted);font-size:12px}
.meta b{color:var(--text);font-weight:600}
.msg{margin:10px 0;border-left:3px solid var(--border);padding:6px 12px;border-radius:4px;background:var(--panel)}
.msg .who{display:flex;gap:8px;align-items:baseline;font-size:11px;color:var(--faint);margin-bottom:4px}
.msg .who .r{font-weight:700;text-transform:uppercase;letter-spacing:.04em}
.msg .body{color:var(--md-text);word-break:break-word;overflow-wrap:anywhere;white-space:normal}
.user{border-left-color:var(--green)}.user .who .r{color:var(--green)}
.assistant{border-left-color:var(--blue)}.assistant .who .r{color:var(--blue)}
.system{border-left-color:var(--faint);opacity:.85}.system .who .r{color:var(--faint)}
.tool{border-left-color:var(--violet)}.tool .who .r{color:var(--violet)}
.result{border-left-color:var(--amber)}.result .who .r{color:var(--amber)}
.ask{border-left-color:var(--red)}.ask .who .r{color:var(--red)}
.compact{border-left-color:var(--amber)}
.reset{border-left:none;background:none;text-align:center;color:var(--faint);font-size:12px;margin:18px 0}
pre{margin:4px 0;padding:8px 10px;background:rgba(0,0,0,.25);border:1px solid var(--border);border-radius:4px;overflow-x:auto;white-space:pre-wrap;word-break:break-word;font-size:12px;line-height:1.45}
code{font-family:inherit}
.md-pre{margin:6px 0}
.md-code{color:var(--md-code);background:var(--md-code-bg);padding:0 4px;border-radius:3px}
.md-h{display:block;font-weight:700;color:var(--md-heading);margin-top:6px}
.md-quote{display:block;border-left:2px solid var(--border);padding-left:8px;color:var(--faint)}
.md-li{display:block;padding-left:8px}
.md-meta-tag{color:var(--faint);font-style:italic}
strong{color:var(--md-strong)}
a{color:var(--blue)}
.syn-keyword{color:var(--syn-keyword)}.syn-string{color:var(--syn-string)}.syn-number{color:var(--syn-number)}
.syn-comment{color:var(--syn-comment);font-style:italic}.syn-function{color:var(--syn-function)}
.ask-q{margin:2px 0;color:var(--text);font-weight:600}
.ask-opts{margin:4px 0 2px;padding-left:18px}
footer{margin-top:28px;color:var(--faint);font-size:11px;text-align:center}
@media print{
:root{--bg:#fff;--panel:#f6f7f8;--border:#d5dade;--text:#1c2228;--muted:#3a434b;--faint:#7a838b;
--blue:#0b62c4;--green:#1a7f37;--amber:#9a6700;--violet:#7a3fc0;--red:#c4302b;--md-code-bg:rgba(11,98,196,.08)}
body{font-size:11px}
pre{background:#f0f2f4;white-space:pre-wrap}
.msg{break-inside:avoid-page}
a{color:var(--blue);text-decoration:none}
@page{margin:14mm}}
`;

export function buildConversationHtml(session: SessionListItem, events: AgentEvent[]): string {
	const blocks = events.map(toBlock).filter((b): b is Block => b !== null);
	const title = session.name || session.working_dir || session.id;
	const first = blocks[0]?.ts;
	const last = blocks[blocks.length - 1]?.ts;
	const meta: string[] = [
		`<span><b>session</b> ${escapeHtml(session.id)}</span>`,
		session.machine_name ? `<span><b>machine</b> ${escapeHtml(session.machine_name)}</span>` : '',
		session.model ? `<span><b>model</b> ${escapeHtml(session.model)}${session.effort ? ` · ${escapeHtml(session.effort)}` : ''}</span>` : '',
		session.working_dir ? `<span><b>cwd</b> ${escapeHtml(session.working_dir)}</span>` : '',
		first ? `<span><b>from</b> ${escapeHtml(fmtTs(first))}</span>` : '',
		last ? `<span><b>to</b> ${escapeHtml(fmtTs(last))}</span>` : '',
		`<span><b>events</b> ${blocks.length}</span>`
	].filter(Boolean);

	const body = blocks
		.map((b) => {
			if (b.role === 'reset') return `<div class="reset">${b.html}</div>`;
			const label = b.label ? `${ROLE_LABEL[b.role]} · ${escapeHtml(b.label)}` : ROLE_LABEL[b.role];
			return `<div class="msg ${b.role}"><div class="who"><span class="r">${label}</span><span>${escapeHtml(fmtTs(b.ts))}</span></div><div class="body">${b.html}</div></div>`;
		})
		.join('\n');

	return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8" />
<meta name="viewport" content="width=device-width, initial-scale=1" />
<title>${escapeHtml(title)} — cctui transcript</title>
<style>${CSS}</style>
</head>
<body>
<div class="page">
<header><h1>${escapeHtml(title)}</h1><div class="meta">${meta.join('')}</div></header>
${body}
<footer>Exported from cctui · ${escapeHtml(new Date().toLocaleString())} · use your browser's Print → Save as PDF for a PDF copy</footer>
</div>
</body>
</html>
`;
}

/** Trigger a client-side download of the built HTML transcript. */
export function downloadConversationHtml(session: SessionListItem, events: AgentEvent[]) {
	const html = buildConversationHtml(session, events);
	const stamp = new Date().toISOString().slice(0, 10);
	const base = (session.name || session.id).replace(/[^\w.-]+/g, '_').slice(0, 60) || 'conversation';
	const blob = new Blob([html], { type: 'text/html;charset=utf-8' });
	const url = URL.createObjectURL(blob);
	const a = document.createElement('a');
	a.href = url;
	a.download = `cctui-${base}-${stamp}.html`;
	document.body.appendChild(a);
	a.click();
	a.remove();
	URL.revokeObjectURL(url);
}
