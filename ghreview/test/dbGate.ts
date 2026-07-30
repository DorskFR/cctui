import { test } from "bun:test";

type SuiteFn = (name: string, fn: () => void) => void;
interface DescribeLike extends SuiteFn {
  skip: SuiteFn;
}

export function dbGate(describe: DescribeLike, databaseUrl: string | undefined): SuiteFn {
  if (databaseUrl) return describe;
  if (process.env.CI) {
    return (name: string) => {
      describe(name, () => {
        test("requires DATABASE_URL under CI", () => {
          throw new Error(`${name}: DATABASE_URL must be set in CI to run DB-backed tests`);
        });
      });
    };
  }
  return describe.skip;
}
