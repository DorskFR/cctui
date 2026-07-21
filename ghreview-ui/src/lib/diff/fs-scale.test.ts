import { describe, expect, it } from "vitest";
import { BASE_FONT_PX, BASE_ROW_HEIGHT, fontPxFor, parseFsScale, rowHeightFor } from "./fs-scale";

describe("parseFsScale", () => {
  it("defaults to 1 for missing/blank/invalid input", () => {
    expect(parseFsScale(null)).toBe(1);
    expect(parseFsScale(undefined)).toBe(1);
    expect(parseFsScale("")).toBe(1);
    expect(parseFsScale("   ")).toBe(1);
    expect(parseFsScale("nope")).toBe(1);
    expect(parseFsScale("0")).toBe(1);
    expect(parseFsScale("-2")).toBe(1);
  });

  it("parses a numeric multiplier, trimming whitespace", () => {
    expect(parseFsScale("1.45")).toBe(1.45);
    expect(parseFsScale("  2 ")).toBe(2);
    expect(parseFsScale("0.7")).toBe(0.7);
  });

  it("clamps to the supported range", () => {
    expect(parseFsScale("0.1")).toBe(0.5);
    expect(parseFsScale("99")).toBe(3);
  });
});

describe("rowHeightFor / fontPxFor", () => {
  it("scales the base row height and rounds to a whole pixel", () => {
    expect(rowHeightFor(1)).toBe(BASE_ROW_HEIGHT);
    expect(rowHeightFor(2)).toBe(40);
    expect(rowHeightFor(0.7)).toBe(14);
    expect(rowHeightFor(1.45)).toBe(29);
  });

  it("never returns a zero row height", () => {
    expect(rowHeightFor(0.001)).toBe(1);
  });

  it("scales the base font size", () => {
    expect(fontPxFor(1)).toBe(BASE_FONT_PX);
    expect(fontPxFor(2)).toBe(24);
  });
});
