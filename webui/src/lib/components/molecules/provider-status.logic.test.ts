import { describe, expect, it } from "vitest";
import type { ProviderStatus } from "$lib/queries";
import {
  componentNames,
  degraded,
  familyLabel,
  indicatorTone,
  isDegraded,
  statusForProvider,
  worstIndicator,
} from "./provider-status.logic";

const status = (p: Partial<ProviderStatus>): ProviderStatus => ({
  family: "anthropic",
  indicator: "none",
  description: "",
  url: "https://status.claude.com",
  components: [],
  incidents: [],
  ...p,
});

describe("provider status", () => {
  it("treats unknown as neither healthy nor degraded", () => {
    expect(isDegraded("unknown")).toBe(false);
    expect(isDegraded("none")).toBe(false);
    expect(isDegraded("minor")).toBe(true);
    expect(worstIndicator([status({ indicator: "unknown" })])).toBeNull();
    expect(indicatorTone("unknown")).toBeNull();
    expect(indicatorTone(null)).toBeNull();
  });

  it("renders nothing when the fetch never answered", () => {
    // A failed or absent /provider-status leaves `data` undefined; the indicator
    // reads that as unknown, which is not a badge.
    expect(degraded(undefined)).toEqual([]);
    expect(worstIndicator(undefined)).toBeNull();
    expect(indicatorTone(worstIndicator(undefined))).toBeNull();
    expect(statusForProvider(undefined, "anthropic")).toBeNull();
  });

  it("keeps only the degraded families", () => {
    const list = [
      status({ family: "anthropic", indicator: "none" }),
      status({ family: "openai", indicator: "major" }),
    ];
    expect(degraded(list).map((s) => s.family)).toEqual(["openai"]);
    expect(degraded(undefined)).toEqual([]);
  });

  it("reports the worst indicator across families", () => {
    expect(
      worstIndicator([status({ indicator: "minor" }), status({ indicator: "critical" })]),
    ).toBe("critical");
    expect(worstIndicator([status({ indicator: "minor" }), status({ indicator: "none" })])).toBe(
      "minor",
    );
    expect(worstIndicator([])).toBeNull();
  });

  it("maps severity to a tone", () => {
    expect(indicatorTone("minor")).toBe("warn");
    expect(indicatorTone("major")).toBe("danger");
    expect(indicatorTone("critical")).toBe("danger");
    expect(indicatorTone("none")).toBeNull();
  });

  it("labels the polled families and passes anything else through", () => {
    expect(familyLabel("anthropic")).toBe("Claude");
    expect(familyLabel("openai")).toBe("Codex");
    expect(familyLabel("whatever")).toBe("whatever");
  });

  it("caps the component list", () => {
    const s = status({
      indicator: "major",
      components: ["a", "b", "c", "d"].map((name) => ({ name, status: "major_outage" })),
    });
    expect(componentNames(s)).toEqual(["a", "b", "c"]);
    expect(componentNames(s, 1)).toEqual(["a"]);
  });

  it("attributes an incident only to the first-party provider of that family", () => {
    const list = [status({ family: "anthropic", indicator: "minor" })];
    expect(statusForProvider(list, "anthropic")?.family).toBe("anthropic");
    expect(statusForProvider(list, "anthropic-compatible")).toBeNull();
    expect(statusForProvider(list, "openai-compatible")).toBeNull();
    expect(statusForProvider(list, "fireworks")).toBeNull();
    expect(statusForProvider(list, "openai")).toBeNull();
  });

  it("does not attribute a healthy family to its own provider", () => {
    const list = [status({ family: "openai", indicator: "none" })];
    expect(statusForProvider(list, "openai")).toBeNull();
  });
});
