import type { SecretScrubPattern } from "$lib/settings.svelte";

export interface ScrubPatternIssue {
  /** 1-based. */
  line: number;
  regex: string;
  message: string;
}

export interface ParsedScrubPatterns {
  patterns: SecretScrubPattern[];
  issues: ScrubPatternIssue[];
}

/** Split the Extra-patterns textarea into savable patterns plus the lines that
 *  don't compile. JS and Rust `regex` syntax differ, so the server stays
 *  authoritative; this only keeps a locally-broken line out of the payload so
 *  one bad pattern can't make every later settings PUT fail. */
export function parseScrubPatterns(text: string): ParsedScrubPatterns {
  const patterns: SecretScrubPattern[] = [];
  const issues: ScrubPatternIssue[] = [];
  const seen = new Set<string>();
  text.split("\n").forEach((raw, i) => {
    const regex = raw.trim();
    if (!regex) return;
    try {
      new RegExp(regex);
    } catch (e) {
      issues.push({
        line: i + 1,
        regex,
        message: e instanceof Error ? e.message : String(e),
      });
      return;
    }
    if (seen.has(regex)) return;
    seen.add(regex);
    patterns.push({ name: "custom", regex, enabled: true });
  });
  return { patterns, issues };
}
