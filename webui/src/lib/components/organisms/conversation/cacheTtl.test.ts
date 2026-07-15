import { describe, it, expect } from "vitest";
import {
	cacheTtlMs,
	isGpt56OrLater,
	ANTHROPIC_TTL_MS,
	OPENAI_GPT56_TTL_MS,
	DEFAULT_TTL_MS,
} from "./cacheTtl";

describe("cacheTtlMs", () => {
	it("Anthropic / Claude Code → 60m regardless of model", () => {
		expect(cacheTtlMs("claude-code", null)).toBe(ANTHROPIC_TTL_MS);
		expect(cacheTtlMs("claude-code", "claude-opus-4-8")).toBe(60 * 60 * 1000);
	});

	it("OpenAI GPT-5.6+ → 30m", () => {
		expect(cacheTtlMs("codex", "gpt-5.6")).toBe(OPENAI_GPT56_TTL_MS);
		expect(cacheTtlMs("codex", "gpt-5.6-codex")).toBe(30 * 60 * 1000);
		expect(cacheTtlMs("openai", "gpt-6")).toBe(OPENAI_GPT56_TTL_MS);
	});

	it("OpenAI below 5.6 falls back to the 5m legacy window", () => {
		expect(cacheTtlMs("codex", "gpt-5.5")).toBe(DEFAULT_TTL_MS);
		expect(cacheTtlMs("codex", "gpt-4.1")).toBe(DEFAULT_TTL_MS);
		expect(cacheTtlMs("codex", null)).toBe(DEFAULT_TTL_MS);
	});

	it("unknown / null adapter → 5m default", () => {
		expect(cacheTtlMs(null, null)).toBe(DEFAULT_TTL_MS);
		expect(cacheTtlMs("something-else", "gpt-6")).toBe(DEFAULT_TTL_MS);
	});
});

describe("isGpt56OrLater", () => {
	it("parses major.minor thresholds", () => {
		expect(isGpt56OrLater("gpt-5.6")).toBe(true);
		expect(isGpt56OrLater("gpt-5.7-codex")).toBe(true);
		expect(isGpt56OrLater("gpt-6")).toBe(true);
		expect(isGpt56OrLater("gpt-5.5")).toBe(false);
		expect(isGpt56OrLater("gpt-4.1")).toBe(false);
		expect(isGpt56OrLater(null)).toBe(false);
		expect(isGpt56OrLater("o3")).toBe(false);
	});
});
