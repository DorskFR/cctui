// biome-ignore-all lint/suspicious/noControlCharactersInRegex: ANSI/C0 stripping requires matching control bytes
// Also imported by webui through the `$ghreview` alias; its ambient types live
// in webui/src/ghreview-embed.d.ts and must follow export changes.

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

export function escapeHtml(value: string): string {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

const ANSI_RE =
  /[\x1B\x9B][[\]()#;?]*(?:(?:(?:(?:;[-a-zA-Z\d/#&.:=?%@~_]+)*|[a-zA-Z\d]+(?:;[-a-zA-Z\d/#&.:=?%@~_]*)*)?\x07)|(?:(?:\d{1,4}(?:;\d{0,4})*)?[\dA-PR-TZcf-ntqry=><~]))/g;
const C0_RE = /[\x00-\x08\x0B-\x1F\x7F]/g;

export function stripAnsi(value: string): string {
  return value.replace(ANSI_RE, "").replace(C0_RE, "");
}

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

export { hljs };

export const LANG_ALIAS: Record<string, string> = {
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

export function looksLikeDiff(value: string): boolean {
  const lines = value.split("\n").filter(Boolean);
  if (lines.length < 2) return false;
  const markedLines = lines.filter((line) => line[0] === "+" || line[0] === "-").length;
  return markedLines >= 1 && markedLines >= lines.length * 0.5;
}

export function highlightDiff(value: string): string {
  return value
    .split("\n")
    .map((line) => {
      const escaped = escapeHtml(line);
      if (line.startsWith("+")) return `<span class="hljs-addition">${escaped}</span>`;
      if (line.startsWith("-")) return `<span class="hljs-deletion">${escaped}</span>`;
      if (line.startsWith("@@")) return `<span class="hljs-meta">${escaped}</span>`;
      return escaped;
    })
    .join("\n");
}

export function highlightCode(rawCode: string, language: string): string {
  const clean = stripAnsi(rawCode);
  const normalized = LANG_ALIAS[language.toLowerCase()] ?? language.toLowerCase();
  if (normalized === "diff" || (!normalized && looksLikeDiff(clean))) return highlightDiff(clean);
  if (normalized && hljs.getLanguage(normalized)) {
    try {
      return hljs.highlight(clean, { language: normalized, ignoreIllegals: true }).value;
    } catch {
      return escapeHtml(clean);
    }
  }
  return escapeHtml(clean);
}
