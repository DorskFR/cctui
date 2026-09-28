import { describe, expect, it } from "vitest";
import { ApiError } from "./api";
import { saveErrorMessage } from "./saveError";

describe("saveErrorMessage", () => {
  it("surfaces a rejected save's own message", () => {
    const err = new ApiError(400, 'invalid scrub regex "*_token": repetition operator missing');
    expect(saveErrorMessage(err)).toBe(
      'invalid scrub regex "*_token": repetition operator missing',
    );
  });

  it("keeps 5xx and network failures generic", () => {
    expect(saveErrorMessage(new ApiError(500, "database error"))).toBeNull();
    expect(saveErrorMessage(new TypeError("Failed to fetch"))).toBeNull();
    expect(saveErrorMessage(undefined)).toBeNull();
  });
});
