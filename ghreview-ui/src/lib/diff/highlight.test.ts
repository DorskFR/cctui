import { describe, expect, it, vi } from "vitest";
import {
  createHighlightCache,
  highlightLine,
  highlightLineCached,
  langForPath,
} from "./highlight";

describe("langForPath", () => {
  it("maps known extensions and gives up on unknown ones", () => {
    expect(langForPath("src/a.ts")).toBe("typescript");
    expect(langForPath("src/a.rs")).toBe("rust");
    expect(langForPath("Makefile")).toBeNull();
    expect(langForPath("src/a.wat")).toBeNull();
  });
});

describe("highlightLine", () => {
  it("escapes html when no language applies", () => {
    expect(highlightLine("<b>&</b>", null)).toBe("&lt;b&gt;&amp;&lt;/b&gt;");
  });

  it("emits hljs spans for a known language", () => {
    expect(highlightLine("const x = 1;", "typescript")).toContain("<span");
  });
});

describe("createHighlightCache", () => {
  it("highlights a given (line, lang) pair exactly once", () => {
    const spy = vi.fn(highlightLine);
    const hl = createHighlightCache(spy);

    const first = hl("const x = 1;", "typescript");
    for (let i = 0; i < 500; i++) expect(hl("const x = 1;", "typescript")).toBe(first);

    expect(spy).toHaveBeenCalledTimes(1);
  });

  it("keys on the language as well as the content", () => {
    const spy = vi.fn(highlightLine);
    const hl = createHighlightCache(spy);

    hl("x", "typescript");
    hl("x", "python");
    hl("x", null);
    hl("x", "typescript");

    expect(spy).toHaveBeenCalledTimes(3);
  });

  it("re-highlights only rows it has not seen before", () => {
    const spy = vi.fn(highlightLine);
    const hl = createHighlightCache(spy);
    const rows = Array.from({ length: 120 }, (_, i) => `line ${i}`);

    for (const r of rows) hl(r, "typescript");
    expect(spy).toHaveBeenCalledTimes(120);

    for (let pass = 0; pass < 10; pass++) for (const r of rows) hl(r, "typescript");
    expect(spy).toHaveBeenCalledTimes(120);
  });

  it("drops the cache once it exceeds its cap instead of growing unbounded", () => {
    const spy = vi.fn(highlightLine);
    const hl = createHighlightCache(spy, 4);

    for (const line of ["a", "b", "c", "d"]) hl(line, null);
    expect(spy).toHaveBeenCalledTimes(4);
    expect(hl("a", null)).toBe("a");
    expect(spy).toHaveBeenCalledTimes(4);

    hl("e", null);
    expect(hl("a", null)).toBe("a");
    expect(spy).toHaveBeenCalledTimes(6);
  });

  it("the shared instance returns the same output as the uncached path", () => {
    const line = "function f(a: number) { return a + 1; }";
    expect(highlightLineCached(line, "typescript")).toBe(highlightLine(line, "typescript"));
    expect(highlightLineCached(line, "typescript")).toBe(highlightLine(line, "typescript"));
  });
});
