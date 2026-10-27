import { describe, expect, it } from "vitest";
import { DEFAULT_VOICE, mergeDefaults, mergeVoice } from "./settings.svelte";

describe("voice preferences", () => {
  it("defaults when absent", () => {
    expect(mergeDefaults({}).voice).toEqual(DEFAULT_VOICE);
  });

  it("clamps unknown and out-of-range values", () => {
    const v = mergeVoice({
      speed: 9,
      inputMode: "shout" as never,
      spokenStyle: "loud" as never,
      voice: "  ",
      bargeIn: false,
      autoPlayVoiceNotes: true,
    });
    expect(v).toEqual({ ...DEFAULT_VOICE, bargeIn: false, autoPlayVoiceNotes: true });
  });

  it("keeps picked devices and language, drops blanks and unknown languages", () => {
    expect(mergeVoice({ micId: " mic-1 ", speakerId: "", language: "fr" })).toMatchObject({
      micId: "mic-1",
      language: "fr",
    });
    const v = mergeVoice({ speakerId: "  ", language: "de" as never });
    expect(v.speakerId).toBeUndefined();
    expect(v.language).toBe("auto");
  });

  it("keeps valid choices", () => {
    const v = mergeVoice({ voice: "af_bella", speed: 1.5, inputMode: "handsfree", spokenStyle: "verbose", autoSend: true });
    expect(v).toMatchObject({ voice: "af_bella", speed: 1.5, inputMode: "handsfree", spokenStyle: "verbose", autoSend: true });
  });
});
