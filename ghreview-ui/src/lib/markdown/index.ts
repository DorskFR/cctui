// biome-ignore-all lint/suspicious/noControlCharactersInRegex: ANSI/C0 stripping requires matching control bytes
/**
 * Safe-ish markdown -> HTML for PR descriptions and comments. Vendored and
 * trimmed from `webui/src/lib/markdown.ts` (ghreview-ui cannot import from webui
 * — the standalone build must stand on its own). Everything is escaped first, so
 * no raw HTML from GitHub survives; we then re-introduce a small, fixed set of
 * tags. Colors are driven by the `.markdown` scope in the consuming components.
 *
 * Trimmed vs. webui: dropped the cctui session-scoped image markers and the
 * code-copy button (no delegated listener here); kept fenced-code highlighting,
 * GFM tables, headings/lists/blockquotes, inline emphasis and autolinking.
 */

import "./markdown.css";

export function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

const ANSI_RE =
  /[\x1B\x9B][[\]()#;?]*(?:(?:(?:(?:;[-a-zA-Z\d/#&.:=?%@~_]+)*|[a-zA-Z\d]+(?:;[-a-zA-Z\d/#&.:=?%@~_]*)*)?\x07)|(?:(?:\d{1,4}(?:;\d{0,4})*)?[\dA-PR-TZcf-ntqry=><~]))/g;
const C0_RE = /[\x00-\x08\x0B-\x1F\x7F]/g;

export function stripAnsi(s: string): string {
  return s.replace(ANSI_RE, "").replace(C0_RE, "");
}

const BLOCK_L = "";
const BLOCK_R = "";

import hljs from "highlight.js/lib/core";
import bashLang from "highlight.js/lib/languages/bash";
import css from "highlight.js/lib/languages/css";
import diffLang from "highlight.js/lib/languages/diff";
import dockerfile from "highlight.js/lib/languages/dockerfile";
import go from "highlight.js/lib/languages/go";
import toml from "highlight.js/lib/languages/ini";
import javascript from "highlight.js/lib/languages/javascript";
import json from "highlight.js/lib/languages/json";
import markdown from "highlight.js/lib/languages/markdown";
import python from "highlight.js/lib/languages/python";
import rust from "highlight.js/lib/languages/rust";
import bash from "highlight.js/lib/languages/shell";
import sql from "highlight.js/lib/languages/sql";
import typescript from "highlight.js/lib/languages/typescript";
import xml from "highlight.js/lib/languages/xml";
import yaml from "highlight.js/lib/languages/yaml";

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

  if (norm === "diff" || (!norm && looksLikeDiff(clean))) return highlightDiff(clean);

  if (norm && hljs.getLanguage(norm)) {
    try {
      return hljs.highlight(clean, { language: norm, ignoreIllegals: true }).value;
    } catch {
      return escapeHtml(clean);
    }
  }
  return escapeHtml(clean);
}

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
      if (line.startsWith("+")) return `<span class="hljs-addition">${esc}</span>`;
      if (line.startsWith("-")) return `<span class="hljs-deletion">${esc}</span>`;
      if (line.startsWith("@@")) return `<span class="hljs-meta">${esc}</span>`;
      return esc;
    })
    .join("\n");
}

function codeBlockHtml(body: string, langAttr: string): string {
  return `<pre class="md-pre"${langAttr}><code>${body}</code></pre>`;
}

const AUTO_L = "\x0E";
const AUTO_R = "\x0F";

function autolinkUrls(s: string): string {
  const saved: string[] = [];
  const stash = (html: string) => `${AUTO_L}${saved.push(html) - 1}${AUTO_R}`;
  s = s.replace(/<a\b[^>]*>[\s\S]*?<\/a>|<code\b[^>]*>[\s\S]*?<\/code>/g, stash);
  s = s.replace(/\bhttps?:\/\/(?:&amp;|[^\s&<>"])+/g, (raw) => {
    let url = raw;
    let trail = "";
    const punct = url.match(/[.,;:!?]+$/);
    if (punct) {
      trail = punct[0];
      url = url.slice(0, -trail.length);
    }
    const close = url.slice(-1);
    if ((close === ")" || close === "]") && !url.includes(close === ")" ? "(" : "[")) {
      trail = close + trail;
      url = url.slice(0, -1);
    }
    if (!url) return raw;
    return `<a href="${url}" target="_blank" rel="noopener noreferrer">${url}</a>${trail}`;
  });
  return s.replace(new RegExp(`${AUTO_L}(\\d+)${AUTO_R}`, "g"), (_m, i) => saved[Number(i)]);
}

function inlineMd(s: string): string {
  s = s.replace(/`([^`]+)`/g, '<code class="md-code">$1</code>');
  s = s.replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>");
  s = s.replace(/(^|[^*])\*([^*\n]+)\*/g, "$1<em>$2</em>");
  s = s.replace(
    /\[([^\]]+)\]\((https?:\/\/[^\s)]+)\)/g,
    '<a href="$2" target="_blank" rel="noopener noreferrer">$1</a>',
  );
  s = autolinkUrls(s);
  return s;
}

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

export function renderMarkdown(src: string, opts: { tables?: boolean } = {}): string {
  const tables = opts.tables !== false;
  src = stripAnsi(src);
  const blocks: string[] = [];

  let s = src.replace(/```([^\n`]*)\n?([\s\S]*?)```/g, (_m, info: string, code: string) => {
    const lang = (info || "").trim().split(/\s+/)[0] ?? "";
    const body = highlightCode(code.replace(/\n$/, ""), lang);
    const cls = lang ? ` data-lang="${escapeHtml(lang)}"` : "";
    const i = blocks.push(codeBlockHtml(body, cls)) - 1;
    return `${BLOCK_L}s${i}${BLOCK_R}`;
  });

  s = escapeHtml(s);

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
          return `<${tag}${a}>${inlineMd(txt.trim())}</${tag}>`;
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

  s = inlineMd(s);
  s = s.replace(/^#{1,6}\s+(.+)$/gm, '<span class="md-h">$1</span>');
  s = s.replace(/^&gt;\s?(.*)$/gm, '<span class="md-quote">$1</span>');
  s = s.replace(/^\s*[-*]\s+(.+)$/gm, '<span class="md-li">• $1</span>');
  s = s.replace(/\n/g, "<br />");

  s = s.replace(new RegExp(`${BLOCK_L}s(\\d+)${BLOCK_R}`, "g"), (_m, i) => blocks[Number(i)]);
  return s;
}
