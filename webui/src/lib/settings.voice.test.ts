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

  it("keeps valid choices", () => {
    const v = mergeVoice({ voice: "af_bella", speed: 1.5, inputMode: "handsfree", spokenStyle: "verbose", autoSend: true });
    expect(v).toMatchObject({ voice: "af_bella", speed: 1.5, inputMode: "handsfree", spokenStyle: "verbose", autoSend: true });
  });
});
