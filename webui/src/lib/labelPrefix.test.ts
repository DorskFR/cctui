
import { expect, it } from "vitest";
import { labelPrefix } from "./labelPrefix";

it("abbreviates visible characters without cutting accents or emoji", () => {
  expect(labelPrefix("agents", 1)).toBe("a");
  expect(labelPrefix("Claudo", 3)).toBe("Cla");
  expect(labelPrefix("e\u0301quipe", 1)).toBe("e\u0301");
  expect(labelPrefix("👩‍💻dev", 1)).toBe("👩‍💻");
  expect(labelPrefix("A", 3)).toBe("A");
});
