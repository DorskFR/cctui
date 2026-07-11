import { describe, expect, test } from "bun:test";
import { randomBytes } from "node:crypto";
import { createSealer, normalizeKey, safeEqual } from "../src/crypto/seal.ts";

describe("seal", () => {
  const key = randomBytes(32).toString("base64");

  test("round-trips a PAT and never stores plaintext", () => {
    const sealer = createSealer(key);
    const pat = `ghp_${"x".repeat(36)}`;
    const sealed = sealer.seal(pat);
    expect(sealed).not.toContain(pat);
    expect(sealed.startsWith("v1:")).toBe(true);
    expect(sealer.open(sealed)).toBe(pat);
  });

  test("produces a fresh nonce each time", () => {
    const sealer = createSealer(key);
    expect(sealer.seal("same")).not.toBe(sealer.seal("same"));
  });

  test("rejects a tampered ciphertext", () => {
    const sealer = createSealer(key);
    const sealed = sealer.seal("secret");
    const parts = sealed.split(":");
    const ct = Buffer.from(parts[3] as string, "base64");
    ct[0] = ct[0] === 0 ? 1 : (ct[0] as number) ^ 0xff;
    const tampered = [parts[0], parts[1], parts[2], ct.toString("base64")].join(":");
    expect(() => sealer.open(tampered)).toThrow();
  });

  test("a different key cannot open the sealed value", () => {
    const sealed = createSealer(key).seal("secret");
    const other = createSealer(randomBytes(32).toString("base64"));
    expect(() => other.open(sealed)).toThrow();
  });

  test("accepts hex, base64 and raw 32-byte keys", () => {
    expect(normalizeKey(randomBytes(32).toString("hex")).length).toBe(32);
    expect(normalizeKey(randomBytes(32).toString("base64")).length).toBe(32);
    expect(normalizeKey("a".repeat(32)).length).toBe(32);
    expect(() => normalizeKey("too-short")).toThrow();
  });

  test("safeEqual compares in constant time by value", () => {
    expect(safeEqual("abc", "abc")).toBe(true);
    expect(safeEqual("abc", "abd")).toBe(false);
    expect(safeEqual("abc", "abcd")).toBe(false);
  });
});
