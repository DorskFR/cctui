/**
 * GitHub-flavoured Markdown for pull-request descriptions and comments.
 *
 * Marked owns the block grammar. This module owns the trust boundary: raw HTML
 * is escaped, links are rebuilt from validated URLs, and images are emitted only
 * for GitHub-managed attachment hosts. Nothing from the source is passed through
 * as an HTML attribute.
 */

import { marked, Renderer } from "marked";
import "./markdown.css";
import "./hljs-tokens.css";
import { escapeHtml, highlightCode, stripAnsi } from "./highlight";

export { escapeHtml, stripAnsi };

export interface MarkdownOptions {
  tables?: boolean;
  /** Canonical repository URL used to resolve relative links and attachment URLs. */
  baseUrl?: string;
}

export function repoBaseUrl(owner?: string, repo?: string): string | undefined {
  if (!owner || !repo) return undefined;
  return `https://github.com/${encodeURIComponent(owner)}/${encodeURIComponent(repo)}/`;
}

function resolveHttpUrl(raw: string, baseUrl?: string): URL | null {
  try {
    const url = baseUrl ? new URL(raw, baseUrl) : new URL(raw);
    return url.protocol === "http:" || url.protocol === "https:" ? url : null;
  } catch {
    return null;
  }
}

function isApprovedImageUrl(url: URL): boolean {
  if (url.protocol !== "https:") return false;
  const host = url.hostname.toLowerCase();
  if (host === "github.com") return url.pathname.startsWith("/user-attachments/assets/");
  return (
    host === "user-images.githubusercontent.com" ||
    host === "private-user-images.githubusercontent.com" ||
    host === "camo.githubusercontent.com"
  );
}

function safeDimension(raw: string | undefined): string | null {
  if (!raw || !/^\d{1,5}$/.test(raw)) return null;
  const value = Number(raw);
  return value >= 1 && value <= 16_384 ? String(value) : null;
}

interface RawImageAttributes {
  src?: string;
  alt?: string;
  width?: string;
  height?: string;
}

/** Parse one standalone raw `<img>` token; all unrecognised attributes are discarded. */
function parseRawImage(raw: string): RawImageAttributes | null {
  const match = raw.trim().match(/^<img\b([^<>]*)\/?\s*>$/i);
  if (!match) return null;
  const attributes: RawImageAttributes = {};
  const attribute = /([:\w-]+)\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'=<>`]+))/g;
  for (const item of match[1].matchAll(attribute)) {
    const name = item[1].toLowerCase();
    if (name === "src" || name === "alt" || name === "width" || name === "height") {
      attributes[name] = item[2] ?? item[3] ?? item[4] ?? "";
    }
  }
  return attributes.src ? attributes : null;
}

function safeImageHtml(
  source: string,
  alt: string,
  baseUrl?: string,
  width?: string,
  height?: string,
): string {
  const url = resolveHttpUrl(source, baseUrl);
  if (!url || !isApprovedImageUrl(url)) return "";
  const safeWidth = safeDimension(width);
  const safeHeight = safeDimension(height);
  const dimensions = `${safeWidth ? ` width="${safeWidth}"` : ""}${safeHeight ? ` height="${safeHeight}"` : ""}`;
  return `<img class="md-img" src="${escapeHtml(url.href)}" alt="${escapeHtml(alt)}"${dimensions} loading="lazy" decoding="async" referrerpolicy="no-referrer" />`;
}

function createRenderer(options: MarkdownOptions): Renderer {
  const renderer = new Renderer();

  renderer.code = ({ text, lang }) => {
    const language = (lang ?? "").trim().split(/\s+/)[0] ?? "";
    const dataLanguage = language ? ` data-lang="${escapeHtml(language)}"` : "";
    return `<pre class="md-pre"${dataLanguage}><code>${highlightCode(text, language)}</code></pre>\n`;
  };

  renderer.link = function ({ href, title, tokens }) {
    const label = this.parser.parseInline(tokens);
    const url = resolveHttpUrl(href, options.baseUrl);
    if (!url) return label;
    const safeTitle = title ? ` title="${escapeHtml(title)}"` : "";
    return `<a href="${escapeHtml(url.href)}"${safeTitle} target="_blank" rel="noopener noreferrer">${label}</a>`;
  };

  renderer.image = ({ href, text }) => safeImageHtml(href, text, options.baseUrl);

  renderer.html = ({ text }) => {
    const image = parseRawImage(text);
    if (image) {
      return safeImageHtml(
        image.src ?? "",
        image.alt ?? "",
        options.baseUrl,
        image.width,
        image.height,
      );
    }
    // Preserve non-image HTML as inert, visible source instead of trusting it.
    return escapeHtml(text);
  };

  return renderer;
}

export function renderMarkdown(source: string, options: MarkdownOptions = {}): string {
  return marked(stripAnsi(source), {
    async: false,
    breaks: false,
    gfm: true,
    renderer: createRenderer(options),
    // Marked's GFM table tokenizer can be disabled for the existing formatting toggle.
    ...(options.tables === false ? { gfm: false } : {}),
  });
}
