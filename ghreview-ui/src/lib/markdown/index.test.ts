import { describe, expect, it } from "vitest";
import { renderMarkdown } from ".";

const REPO_BASE = "https://github.com/example/project/";
const ATTACHMENT = "https://github.com/user-attachments/assets/01234567-89ab-cdef-0123-456789abcdef";

describe("renderMarkdown", () => {
  it("renders compact semantic GFM blocks instead of line-break approximations", () => {
    const html = renderMarkdown(
      `# Summary

An ordinary paragraph with **strong text** and \`inline code\`.

- First item that wraps
  onto a continuation line
  - Nested item
- Second item

1. First step
2. Second step

- [x] Complete
- [ ] Pending

| Name | State |
| --- | :---: |
| Parser | Ready |

\`\`\`ts
const ready = true;
\`\`\``,
      { baseUrl: REPO_BASE },
    );

    expect(html).toContain("<h1>Summary</h1>");
    expect(html).toContain("<p>An ordinary paragraph");
    expect(html).toContain("<ul>");
    expect(html).toContain("<ol>");
    expect(html).toContain("Nested item");
    expect(html).toContain('type="checkbox"');
    expect(html).toContain("<table>");
    expect(html).toContain('<pre class="md-pre" data-lang="ts"><code>');
    expect(html).toContain('<span class="hljs-keyword">const</span>');
    expect(html).not.toContain("<br><br>");
    expect(html).not.toContain('class="md-li"');
  });

  it("renders Markdown and raw GitHub attachment images with reconstructed attributes", () => {
    const html = renderMarkdown(
      `![A safe attachment](${ATTACHMENT})

<img width="11512" height="494" alt="A second attachment" src="${ATTACHMENT}" style="position:fixed" onerror="alert(1)" />`,
      { baseUrl: REPO_BASE },
    );

    expect(html.match(/<img /g)).toHaveLength(2);
    expect(html).toContain(`src="${ATTACHMENT}"`);
    expect(html).toContain('alt="A safe attachment"');
    expect(html).toContain('width="11512" height="494"');
    expect(html).toContain('loading="lazy" decoding="async" referrerpolicy="no-referrer"');
    expect(html).not.toContain("style=");
    expect(html).not.toContain("onerror=");
  });

  it("accepts only bounded integer image dimensions", () => {
    const html = renderMarkdown(
      `<img width="99999" height="50%" src="${ATTACHMENT}" alt="Attachment" />`,
      { baseUrl: REPO_BASE },
    );

    expect(html).toContain("<img");
    expect(html).not.toContain("width=");
    expect(html).not.toContain("height=");
  });

  it.each([
    "https://tracker.example/pixel.png",
    "http://github.com/user-attachments/assets/not-secure",
    "data:image/png;base64,AAAA",
    "javascript:alert(1)",
    "blob:https://github.com/id",
  ])("omits an unapproved image source: %s", (source) => {
    const markdownImage = renderMarkdown(`![unsafe](${source})`, { baseUrl: REPO_BASE });
    const rawImage = renderMarkdown(`<img src="${source}" onerror="alert(1)" />`, {
      baseUrl: REPO_BASE,
    });

    expect(markdownImage).not.toContain("<img");
    expect(rawImage).not.toContain("<img");
    expect(rawImage).not.toContain("onerror");
  });

  it("escapes non-image raw HTML and cannot break out through image alt text", () => {
    const raw = renderMarkdown("<script>alert(1)</script>", { baseUrl: REPO_BASE });
    const image = renderMarkdown(`![&quot; onerror=&quot;alert(1)](${ATTACHMENT})`, {
      baseUrl: REPO_BASE,
    });

    expect(raw).toContain("&lt;script&gt;");
    expect(raw).not.toContain("<script>");
    expect(image).toContain("<img");
    expect(image).not.toContain('alt="" onerror=');
  });

  it("validates links and resolves relative links against explicit repository context", () => {
    const html = renderMarkdown(
      "[guide](docs/guide.md) [site](https://example.org/path) [unsafe](javascript:alert(1))",
      { baseUrl: REPO_BASE },
    );

    expect(html).toContain('href="https://github.com/example/project/docs/guide.md"');
    expect(html).toContain('href="https://example.org/path"');
    expect(html).toContain('target="_blank" rel="noopener noreferrer"');
    expect(html).not.toContain("javascript:");
    expect(html).toContain("unsafe");
  });

  it("does not guess a base for relative links", () => {
    const html = renderMarkdown("[guide](docs/guide.md)");
    expect(html).toContain("guide");
    expect(html).not.toContain("href=");
  });

  it("keeps legacy GitHub-managed image hosts available", () => {
    for (const source of [
      "https://user-images.githubusercontent.com/1/example.png",
      "https://private-user-images.githubusercontent.com/1/example.png",
      "https://camo.githubusercontent.com/abcdef",
    ]) {
      expect(renderMarkdown(`![attachment](${source})`)).toContain("<img");
    }
  });
});
