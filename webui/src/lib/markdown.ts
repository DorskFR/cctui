/**
 * Safe-ish markdown -> HTML for chat text bubbles. Everything is
 * escaped first, so no raw HTML from the model survives; we then re-introduce a
 * small, fixed set of tags. All colors are CSS-variable driven (see
 * `--md-*` / `--syn-*` in variables.css) so themes adapt.
 */

import {
  escapeHtml,
  highlightCode,
  stripAnsi,
} from "$ghreview/lib/markdown/highlight";

export { escapeHtml, stripAnsi };

// Sentinels for placeholder protection — characters that never appear in source
// text or in our escaped HTML, so restore passes can't collide with content.
const BLOCK_L = "";
const BLOCK_R = "";

// Harness / system pseudo-tags that sometimes leak into model text as literal
// markup. We render them as a muted inline chip rather than dropping them.
const PSEUDO_TAG =
  /&lt;(\/?(?:system[- ]message|system-reminder|task-notification|command-name|command-message|local-command[^&]*|bash-input|bash-stdout|bash-stderr)[^&]*?)&gt;/gi;

// Wrap a highlighted code body in a positioned figure carrying a copy button.
// The button is plain markup; a single delegated listener
// (installCodeCopy, src/lib/codecopy.ts) handles the click for every block,
// including those rendered through {@html} in messages and tool-call panes.
function codeBlockHtml(body: string, langAttr: string): string {
  return (
    `<div class="md-pre-wrap">` +
    `<button class="md-copy" type="button" aria-label="Copy code" title="Copy code">` +
    `<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>` +
    `</button>` +
    `<pre class="md-pre"${langAttr}><code>${body}</code></pre>` +
    `</div>`
  );
}

// ── Markdown ────────────────────────────────────────────────────────────────

// Sentinels for the autolink protect/restore pass — control chars that never
// survive into source text (stripAnsi runs once up front in renderMarkdown) and
// are fully balanced within `autolinkUrls`, so they never leak into the output.
const AUTO_L = "\x0E";
const AUTO_R = "\x0F";

// Turn bare http(s) URLs into clickable links. Runs LAST in inlineMd, after the
// `[text](url)` and inline-code passes, so it can protect already-linked URLs
// and code spans from being re-linked. Operates on already-escaped text, where a
// URL's `&` shows as `&amp;`: that entity is allowed mid-URL (query strings),
// while the other entities (`&lt;`/`&gt;`/`&quot;`) terminate the match so a
// trailing escaped delimiter isn't swallowed into the href.
function autolinkUrls(s: string): string {
  const saved: string[] = [];
  const stash = (html: string) => `${AUTO_L}${saved.push(html) - 1}${AUTO_R}`;
  // Keep existing anchors and inline-code spans out of the autolinker.
  s = s.replace(
    /<a\b[^>]*>[\s\S]*?<\/a>|<code\b[^>]*>[\s\S]*?<\/code>/g,
    stash,
  );
  s = s.replace(/\bhttps?:\/\/(?:&amp;|[^\s&<>"])+/g, (raw) => {
    let url = raw;
    let trail = "";
    // Trailing sentence punctuation usually isn't part of the URL.
    const punct = url.match(/[.,;:!?]+$/);
    if (punct) {
      trail = punct[0];
      url = url.slice(0, -trail.length);
    }
    // A closing bracket with no matching opener inside the URL is sentence
    // punctuation too (e.g. "(see https://x.com)"); keep balanced ones
    // (e.g. Wikipedia "..._(disambiguation)").
    const close = url.slice(-1);
    if (
      (close === ")" || close === "]") &&
      !url.includes(close === ")" ? "(" : "[")
    ) {
      trail = close + trail;
      url = url.slice(0, -1);
    }
    if (!url) return raw;
    return `<a href="${url}" target="_blank" rel="noopener noreferrer">${url}</a>${trail}`;
  });
  return s.replace(
    new RegExp(`${AUTO_L}(\\d+)${AUTO_R}`, "g"),
    (_m, i) => saved[Number(i)],
  );
}

// Where agent-linked local paths resolve. Both ids are required: the route
// authorises and scopes the read against the session, not the machine.
export interface LocalFileLinks {
  machineId: string;
  sessionId: string;
}

/** Same-origin URL that serves `path` off `links.machineId` (see
 * `GET /api/v1/machines/{id}/fs/file`). */
export function localFileHref(path: string, links: LocalFileLinks): string {
  const href =
    `/api/v1/machines/${encodeURIComponent(links.machineId)}/fs/file` +
    `?path=${encodeURIComponent(path)}`;
  return `${href}&session_id=${encodeURIComponent(links.sessionId)}`;
}

// An absolute (`/a/b.ext`) or home-relative (`~/a/b.ext`) path with a file
// extension. Runs on escaped text, so `&`, `<`, `>`, quotes never appear in a
// path; the left boundary is start / whitespace / an opener / an entity's `;` /
// one of our own tags' `>`, so `</code>` or a URL's path part never match.
const LOCAL_PATH =
  /(^|[\s([;>])(~?\/(?:[A-Za-z0-9_.@+%-]+\/)*[A-Za-z0-9_.@+%-]+\.[A-Za-z0-9]{1,8})(?=[\s)\],;:!?<]|\.(?:\s|$)|$)/g;

// Turn local file paths into links to the machine-scoped read-file route.
// Runs after autolinkUrls so anchors (and the paths inside their URLs) are
// stashed out of reach first.
function linkifyLocalPaths(s: string, links: LocalFileLinks): string {
  const saved: string[] = [];
  const stash = (html: string) => `${AUTO_L}${saved.push(html) - 1}${AUTO_R}`;
  s = s.replace(/<a\b[^>]*>[\s\S]*?<\/a>/g, stash);
  s = s.replace(LOCAL_PATH, (_m, lead: string, path: string) => {
    const href = localFileHref(path, links);
    const name = escapeHtml(path.slice(path.lastIndexOf("/") + 1));
    return `${lead}<a class="md-file" href="${href}" data-file-name="${name}" rel="noopener noreferrer">${path}</a>`;
  });
  return s.replace(
    new RegExp(`${AUTO_L}(\\d+)${AUTO_R}`, "g"),
    (_m, i) => saved[Number(i)],
  );
}

// Inline markdown passes (code, bold, italic, links) shared between the main
// body render and table-cell rendering. Operates on already-escaped text.
function inlineMd(s: string, links?: LocalFileLinks): string {
  // inline code
  s = s.replace(/`([^`]+)`/g, '<code class="md-code">$1</code>');
  // bold
  s = s.replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>");
  // italic (avoid touching ** already consumed)
  s = s.replace(/(^|[^*])\*([^*\n]+)\*/g, "$1<em>$2</em>");
  // links [text](url)
  s = s.replace(
    /\[([^\]]+)\]\((https?:\/\/[^\s)]+)\)/g,
    '<a href="$2" target="_blank" rel="noopener noreferrer">$1</a>',
  );
  // bare URLs -> links (after the markdown-link pass so they aren't double-linked)
  s = autolinkUrls(s);
  if (links) s = linkifyLocalPaths(s, links);
  return s;
}

// Split a GFM table row into cells. Drops the leading/trailing pipe and honors
// backslash-escaped pipes (`\|`) inside cell content.
function splitRow(row: string): string[] {
  const cells: string[] = [];
  let cur = "";
  const t = row.trim().replace(/^\|/, "").replace(/\|$/, "");
  for (let i = 0; i < t.length; i++) {
    if (t[i] === "\\" && t[i + 1] === "|") {
      cur += "|";
      i++;
    } else if (t[i] === "|") {
      cells.push(cur);
      cur = "";
    } else {
      cur += t[i];
    }
  }
  cells.push(cur);
  return cells;
}

// Agent-posted image markers. The daemon rewrites a message's
// `![alt](/abs/path.png)` into `![alt](cctui-img://<id>)`; only THIS scheme is
// turned into an <img>, served from the session-scoped, cookie-authed blob
// endpoint. Remote/model-authored URLs stay escaped (the XSS/track guard). The
// id is a server-minted uuid, so it is constrained to `[A-Za-z0-9-]` and the
// alt/session id are attribute-escaped before they reach the DOM.
const CCTUI_IMG = /!\[([^\]]*)\]\(cctui-img:\/\/([A-Za-z0-9-]+)\)/g;

function imageMarkerHtml(alt: string, id: string, sessionId: string): string {
  const src = `/api/v1/sessions/${encodeURIComponent(sessionId)}/images/${encodeURIComponent(id)}`;
  const altAttr = escapeHtml(alt);
  return (
    `<img class="md-img" src="${src}" alt="${altAttr}" loading="lazy" ` +
    `data-lightbox="${src}" title="Click to open full image" />`
  );
}

export function renderMarkdown(
  src: string,
  opts: { tables?: boolean; sessionId?: string; machineId?: string } = {},
): string {
  const links: LocalFileLinks | undefined =
    opts.machineId && opts.sessionId
    ? { machineId: opts.machineId, sessionId: opts.sessionId }
    : undefined;
  // Render GFM tables as real <table>s by default; when `tables` is false
  // leave the pipe rows as plain text.
  const tables = opts.tables !== false;
  // Strip terminal control sequences before any structural parsing.
  src = stripAnsi(src);
  // Protect fenced code blocks before escaping the rest.
  const blocks: string[] = [];

  // Protect image markers into the block table so the raw <img> survives the
  // escape + inline passes intact (same mechanism as code blocks). Only when a
  // session id is in scope — without it (export, plan/ask previews) the marker
  // is left to be escaped as plain text, which degrades safely.
  if (opts.sessionId) {
    const sid = opts.sessionId;
    src = src.replace(CCTUI_IMG, (_m, alt: string, id: string) => {
      const i = blocks.push(imageMarkerHtml(alt, id, sid)) - 1;
      return `${BLOCK_L}s${i}${BLOCK_R}`;
    });
  }
  let s = src.replace(
    /```([^\n`]*)\n?([\s\S]*?)```/g,
    (_m, info: string, code: string) => {
      const lang = (info || "").trim().split(/\s+/)[0] ?? "";
      const body = highlightCode(code.replace(/\n$/, ""), lang);
      const cls = lang ? ` data-lang="${escapeHtml(lang)}"` : "";
      const i = blocks.push(codeBlockHtml(body, cls)) - 1;
      // 's'-prefixed for the same reason as the slot placeholders in
      // highlightCode: keep the bare index out of reach of digit-matching passes.
      return `${BLOCK_L}s${i}${BLOCK_R}`;
    },
  );

  s = escapeHtml(s);

  // GFM tables: a header row, a delimiter row (---|:--:|--- with
  // optional alignment colons), then ≥1 body rows. Detected on the escaped text
  // (so cell content stays safe) and rendered to a real <table>. Cells are run
  // through the inline passes below via a placeholder so bold/code/links inside
  // cells still render; we stash the whole table to keep it clear of the
  // list/blockquote/line-break passes.
  if (tables)
    s = s.replace(
      /(?:^|\n)([ \t]*\|.+\|[ \t]*)\n([ \t]*\|(?:[ \t]*:?-+:?[ \t]*\|)+[ \t]*)\n((?:[ \t]*\|.*\|[ \t]*(?:\n|$))+)/g,
      (_m, header: string, delim: string, body: string) => {
        const aligns = splitRow(delim).map((c) => {
          const l = c.startsWith(":");
          const r = c.endsWith(":");
          return r && l ? "center" : r ? "right" : l ? "left" : "";
        });
        const cell = (txt: string, i: number, tag: "th" | "td") => {
          const a = aligns[i] ? ` style="text-align:${aligns[i]}"` : "";
          return `<${tag}${a}>${inlineMd(txt.trim(), links)}</${tag}>`;
        };
        const head = `<tr>${splitRow(header)
          .map((c, i) => cell(c, i, "th"))
          .join("")}</tr>`;
        const rows = body
          .split("\n")
          .filter((r) => r.trim())
          .map(
            (r) =>
              `<tr>${splitRow(r)
                .map((c, i) => cell(c, i, "td"))
                .join("")}</tr>`,
          )
          .join("");
        const i =
          blocks.push(
            `<table class="md-table"><thead>${head}</thead><tbody>${rows}</tbody></table>`,
          ) - 1;
        return `${BLOCK_L}s${i}${BLOCK_R}`;
      },
    );

  // Leaked harness pseudo-tags -> muted chip (don't show as broken text).
  s = s.replace(PSEUDO_TAG, '<span class="md-meta-tag">&lt;$1&gt;</span>');

  // inline emphasis/code/links
  s = inlineMd(s, links);
  // headings -> styled bold line
  s = s.replace(/^#{1,6}\s+(.+)$/gm, '<span class="md-h">$1</span>');
  // blockquote
  s = s.replace(/^&gt;\s?(.*)$/gm, '<span class="md-quote">$1</span>');
  // unordered list items
  s = s.replace(/^\s*[-*]\s+(.+)$/gm, '<span class="md-li">• $1</span>');
  // line breaks
  s = s.replace(/\n/g, "<br />");

  // restore code blocks
  s = s.replace(
    new RegExp(`${BLOCK_L}s(\\d+)${BLOCK_R}`, "g"),
    (_m, i) => blocks[Number(i)],
  );
  return s;
}

/** Highlight a standalone code/JSON string for a <pre> bubble (tool calls,
 * results). Returns escaped, span-wrapped HTML for use with {@html}. */
export function highlightBlock(raw: string, lang = ""): string {
  return highlightCode(raw, lang);
}

export function prettyJson(v: unknown): string {
  try {
    return JSON.stringify(v, null, 2);
  } catch {
    return String(v);
  }
}
