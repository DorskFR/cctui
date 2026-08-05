import { describe, expect, it } from "vitest";
import { highlightLine, highlightLineCached, langForPath } from "../diff/highlight";

const PAYLOADS = [
  `<script>alert(1)</script>`,
  `const x = "<script>alert(1)</script>";`,
  `<img src=x onerror=alert(1)>`,
  `<img src="x" onerror="alert(1)" />`,
  `<a href="javascript:alert(1)">click</a>`,
  `<a href="data:text/html,<script>alert(1)</script>">x</a>`,
  `# heading <img src=x onerror=alert(1)>`,
  `<svg/onload=alert(1)>`,
  `- item <a href="vbscript:msgbox(1)">y</a>`,
  `"><script>alert(document.cookie)</script>`,
  `<iframe src=javascript:alert(1)></iframe>`,
  `let s = '<img src=x ONERROR=alert(1)>'`,
];

function assertInert(html: string, input: string, lang: string | null) {
  const host = document.createElement("div");
  host.innerHTML = html;
  expect(host.querySelectorAll("script").length, `<script> [${lang}] ${input}`).toBe(0);
  for (const el of host.querySelectorAll("*")) {
    for (const attr of el.attributes) {
      expect(attr.name.startsWith("on"), `handler ${attr.name} [${lang}] ${input}`).toBe(false);
    }
    const href = el.getAttribute("href");
    if (href !== null) {
      expect(/^https?:\/\//i.test(href), `bad href "${href}" [${lang}] ${input}`).toBe(true);
    }
  }
}

describe("DiffView highlight sink is XSS-safe on both paths", () => {
  const fallbackLangs: (string | null)[] = [null, langForPath("Makefile"), "no-such-lang"];
  const hljsLangs = ["typescript", "javascript", "xml", "json", "python"];

  for (const p of PAYLOADS) {
    it(`renders inert (fallback path): ${JSON.stringify(p).slice(0, 44)}`, () => {
      for (const lang of fallbackLangs) {
        assertInert(highlightLine(p, lang), p, lang);
        assertInert(highlightLineCached(p, lang), p, lang);
      }
    });

    it(`renders inert (hljs path): ${JSON.stringify(p).slice(0, 44)}`, () => {
      for (const lang of hljsLangs) {
        assertInert(highlightLine(p, lang), p, lang);
        assertInert(highlightLineCached(p, lang), p, lang);
      }
    });
  }

  it("still highlights a benign line into hljs spans (path is actually exercised)", () => {
    const html = highlightLine(`const answer = 42;`, "typescript");
    expect(html).toContain("<span");
    assertInert(html, "benign", "typescript");
  });

  it("escapes a plain hostile line verbatim on the fallback path", () => {
    expect(highlightLine(`<img src=x onerror=alert(1)>`, null)).toBe(
      "&lt;img src=x onerror=alert(1)&gt;",
    );
  });
});
