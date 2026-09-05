/**
 * Safe-ish markdown -> HTML for chat text bubbles. Everything is
 * escaped first, so no raw HTML from the model survives; we then re-introduce a
 * small, fixed set of tags. All colors are CSS-variable driven (see
 * `--md-*` / `--syn-*` in variables.css) so themes adapt.
 *
 * Adds over the legacy renderer:
 *  - per-language fenced-code highlighting (lang from the info-string)
 *  - Claude-terminal feel: grayish prose, bold = bright, `inline code` = blue
 *  - headings, lists, blockquotes
 *  - leaked `<system message>` / harness pseudo-tags rendered as muted markup
 *    instead of being dropped or shown as broken text.
 */

export function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

// Terminal output (diffs, tool stdout) often carries ANSI escape sequences. They
// aren't HTML-escaped by escapeHtml, so left in place they leak into the DOM as
// raw control bytes — visible as garbled `28→29`-style artifacts that turn
// pink/red when copied into a terminal. Strip the SGR/CSI/OSC sequences and any
// stray C0 control chars (keeping \t and \n) before rendering. The ANSI pattern
// is the well-worn `ansi-regex` one (ESC / CSI introducers + parameter bytes).
// eslint-disable-next-line no-control-regex
const ANSI_RE =
  /[\x1B\x9B][[\]()#;?]*(?:(?:(?:(?:;[-a-zA-Z\d/#&.:=?%@~_]+)*|[a-zA-Z\d]+(?:;[-a-zA-Z\d/#&.:=?%@~_]*)*)?\x07)|(?:(?:\d{1,4}(?:;\d{0,4})*)?[\dA-PR-TZcf-ntqry=><~]))/g;
// C0 control chars except tab (\x09) and newline (\x0A), plus DEL (\x7F).
// eslint-disable-next-line no-control-regex
const C0_RE = /[\x00-\x08\x0B-\x1F\x7F]/g;

export function stripAnsi(s: string): string {
  return s.replace(ANSI_RE, "").replace(C0_RE, "");
}

// Sentinels for placeholder protection — characters that never appear in source
// text or in our escaped HTML, so restore passes can't collide with content.
const BLOCK_L = "";
const BLOCK_R = "";

// Harness / system pseudo-tags that sometimes leak into model text as literal
// markup. We render them as a muted inline chip rather than dropping them.
const PSEUDO_TAG =
  /&lt;(\/?(?:system[- ]message|system-reminder|task-notification|command-name|command-message|local-command[^&]*|bash-input|bash-stdout|bash-stderr)[^&]*?)&gt;/gi;

// ── Syntax highlighting ─────────────────────────────────────────────────────
// Grammar-driven highlighting via highlight.js. We register only the languages
// we care about (the common-set bundle), so the dep stays lean. highlight.js emits
// already-escaped HTML with `hljs-*` token classes; we map those to the existing
// `--syn-*` theme variables in CSS so dark/light/sepia themes still drive the
// colors (and the standalone export's baked palette keeps working). Unknown
// languages and diffs fall back to plain escaped text — still safe, just flat.

import hljs from "highlight.js/lib/core";
import javascript from "highlight.js/lib/languages/javascript";
import typescript from "highlight.js/lib/languages/typescript";
import python from "highlight.js/lib/languages/python";
import rust from "highlight.js/lib/languages/rust";
import go from "highlight.js/lib/languages/go";
import bash from "highlight.js/lib/languages/shell";
import bashLang from "highlight.js/lib/languages/bash";
import json from "highlight.js/lib/languages/json";
import yaml from "highlight.js/lib/languages/yaml";
import xml from "highlight.js/lib/languages/xml";
import css from "highlight.js/lib/languages/css";
import sql from "highlight.js/lib/languages/sql";
import dockerfile from "highlight.js/lib/languages/dockerfile";
import diffLang from "highlight.js/lib/languages/diff";
import markdown from "highlight.js/lib/languages/markdown";
import toml from "highlight.js/lib/languages/ini";

hljs.registerLanguage("javascript", javascript);
hljs.registerLanguage("typescript", typescript);
hljs.registerLanguage("python", python);
hljs.registerLanguage("rust", rust);
hljs.registerLanguage("go", go);
hljs.registerLanguage("shell", bash);
hljs.registerLanguage("bash", bashLang);
hljs.registerLanguage("json", json);
hljs.registerLanguage("yaml", yaml);
hljs.registerLanguage("xml", xml);
hljs.registerLanguage("css", css);
hljs.registerLanguage("sql", sql);
hljs.registerLanguage("dockerfile", dockerfile);
hljs.registerLanguage("diff", diffLang);
hljs.registerLanguage("markdown", markdown);
hljs.registerLanguage("ini", toml);

/** The shared highlight.js core with our lean language set already registered.
 * Re-exported so the diff viewer (GH-VIEW-3) can highlight individual rendered
 * lines without registering its own duplicate language bundle. */
export { hljs };

/** Resolve a fenced-code info-string / language hint to a registered highlight.js
 * language name, applying our alias table (`ts`→`typescript`, …). Returns the
 * canonical name when known, else `null`. */
export function resolveLang(lang: string): string | null {
  const norm = LANG_ALIAS[lang.toLowerCase()] ?? lang.toLowerCase();
  return norm && hljs.getLanguage(norm) ? norm : null;
}

const LANG_ALIAS: Record<string, string> = {
  js: "javascript",
  jsx: "javascript",
  mjs: "javascript",
  cjs: "javascript",
  ts: "typescript",
  tsx: "typescript",
  py: "python",
  rs: "rust",
  golang: "go",
  sh: "shell",
  zsh: "shell",
  yml: "yaml",
  html: "xml",
  svg: "xml",
  md: "markdown",
  toml: "ini",
  docker: "dockerfile",
  patch: "diff",
};

function highlightCode(rawCode: string, lang: string): string {
  const clean = stripAnsi(rawCode);
  const norm = LANG_ALIAS[lang.toLowerCase()] ?? lang.toLowerCase();

  // Diff blocks (our pretty-diff path passes `lang: ''` but the body is `+`/`-`
  // prefixed lines) and explicit diff/patch: line-color them ourselves so the
  // classic green-add / red-remove reads at a glance, independent of grammar.
  if (norm === "diff" || (!norm && looksLikeDiff(clean)))
    return highlightDiff(clean);

  if (norm && hljs.getLanguage(norm)) {
    try {
      return hljs.highlight(clean, { language: norm, ignoreIllegals: true })
        .value;
    } catch {
      /* fall through to plain */
    }
  }
  // Unknown / no language: escaped plain text (flat, but safe).
  return escapeHtml(clean);
}

// Heuristic: a body where most non-blank lines start with +/-/space (and at
// least one +/- line) is a unified diff even without a `diff` info-string.
function looksLikeDiff(s: string): boolean {
  const lines = s.split("\n").filter((l) => l.length);
  if (lines.length < 2) return false;
  let marked = 0;
  for (const l of lines) if (l[0] === "+" || l[0] === "-") marked++;
  return marked >= 1 && marked >= lines.length * 0.5;
}

function highlightDiff(s: string): string {
  return s
    .split("\n")
    .map((line) => {
      const esc = escapeHtml(line);
      if (line.startsWith("+"))
        return `<span class="hljs-addition">${esc}</span>`;
      if (line.startsWith("-"))
        return `<span class="hljs-deletion">${esc}</span>`;
      if (line.startsWith("@@")) return `<span class="hljs-meta">${esc}</span>`;
      return esc;
    })
    .join("\n");
}

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

// Where agent-linked local paths resolve: the machine the session runs on
// (the read-file route is machine-scoped; the session widens its allow-list).
export interface LocalFileLinks {
  machineId: string;
  sessionId?: string;
}

/** Same-origin URL that serves `path` off `links.machineId` (see
 * `GET /api/v1/machines/{id}/fs/file`). */
export function localFileHref(path: string, links: LocalFileLinks): string {
  let href =
    `/api/v1/machines/${encodeURIComponent(links.machineId)}/fs/file` +
    `?path=${encodeURIComponent(path)}`;
  if (links.sessionId) href += `&session_id=${encodeURIComponent(links.sessionId)}`;
  return href;
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
  // Local paths become links only when the machine to read them from is known.
  const links: LocalFileLinks | undefined = opts.machineId
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
