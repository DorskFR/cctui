import { createCipheriv, createDecipheriv, randomBytes, timingSafeEqual } from "node:crypto";

const VERSION = "v1";
const NONCE_BYTES = 12;
const KEY_BYTES = 32;

export interface Sealer {
  seal: (plaintext: string) => string;
  open: (sealed: string) => string;
}

export function normalizeKey(raw: string): Buffer {
  const attempts: Buffer[] = [];
  if (/^[0-9a-fA-F]+$/.test(raw) && raw.length === KEY_BYTES * 2) {
    attempts.push(Buffer.from(raw, "hex"));
  }
  try {
    attempts.push(Buffer.from(raw, "base64"));
  } catch {}
  attempts.push(Buffer.from(raw, "utf8"));
  const exact = attempts.find((b) => b.length === KEY_BYTES);
  if (exact) return exact;
  throw new Error(
    "GHREVIEW_SEAL_KEY must decode to 32 bytes (64 hex chars, base64, or a 32-byte string)",
  );
}

export function createSealer(rawKey: string): Sealer {
  const key = normalizeKey(rawKey);
  return {
    seal(plaintext: string): string {
      const nonce = randomBytes(NONCE_BYTES);
      const cipher = createCipheriv("aes-256-gcm", key, nonce);
      const ct = Buffer.concat([cipher.update(plaintext, "utf8"), cipher.final()]);
      const tag = cipher.getAuthTag();
      return [
        VERSION,
        nonce.toString("base64"),
        tag.toString("base64"),
        ct.toString("base64"),
      ].join(":");
    },
    open(sealed: string): string {
      const parts = sealed.split(":");
      if (parts.length !== 4 || parts[0] !== VERSION) {
        throw new Error("malformed sealed value");
      }
      const nonce = Buffer.from(parts[1] as string, "base64");
      const tag = Buffer.from(parts[2] as string, "base64");
      const ct = Buffer.from(parts[3] as string, "base64");
      const decipher = createDecipheriv("aes-256-gcm", key, nonce);
      decipher.setAuthTag(tag);
      return Buffer.concat([decipher.update(ct), decipher.final()]).toString("utf8");
    },
  };
}

export function safeEqual(a: string, b: string): boolean {
  const ba = Buffer.from(a);
  const bb = Buffer.from(b);
  if (ba.length !== bb.length) return false;
  return timingSafeEqual(ba, bb);
}
