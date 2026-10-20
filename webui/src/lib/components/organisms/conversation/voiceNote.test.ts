import { describe, expect, it } from "vitest";
import { VOICE_TOOL, claimAnnounce, formatClock, voiceNoteUrl } from "./voiceNote";

describe("voice notes", () => {
  it("builds the session-scoped audio url", () => {
    expect(voiceNoteUrl("s 1", "n1")).toBe("/api/v1/sessions/s%201/voice-notes/n1");
  });

  it("announces a fresh note once and never a reloaded one", () => {
    const now = 1_000_000;
    expect(claimAnnounce("a", now - 1000, now)).toBe(true);
    expect(claimAnnounce("a", now - 1000, now)).toBe(false);
    expect(claimAnnounce("b", now - 3_600_000, now)).toBe(false);
  });

  it("formats a clock", () => {
    expect(formatClock(0)).toBe("0:00");
    expect(formatClock(75.6)).toBe("1:15");
    expect(formatClock(Number.NaN)).toBe("0:00");
  });

  it("matches the speak tool under any MCP prefix", () => {
    expect(VOICE_TOOL.test("mcp__cctui__CctuiSpeak")).toBe(true);
    expect(VOICE_TOOL.test("CctuiSpeak")).toBe(true);
    expect(VOICE_TOOL.test("CctuiSpeakers")).toBe(false);
  });
});
