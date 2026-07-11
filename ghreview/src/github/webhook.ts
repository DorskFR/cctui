import { createHmac, timingSafeEqual } from "node:crypto";

export function signPayload(secret: string, body: string): string {
  const mac = createHmac("sha256", secret).update(body).digest("hex");
  return `sha256=${mac}`;
}

export function verifySignature(secret: string, body: string, signature: string | null): boolean {
  if (!signature) return false;
  const expected = signPayload(secret, body);
  const a = Buffer.from(expected);
  const b = Buffer.from(signature);
  if (a.length !== b.length) return false;
  return timingSafeEqual(a, b);
}
