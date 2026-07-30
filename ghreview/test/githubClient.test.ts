import { describe, expect, test } from "bun:test";
import {
  REQUEST_RETRIES,
  REQUEST_TIMEOUT_MS,
  retryingFetch,
  USER_AGENT,
} from "../src/github/client.ts";

const noSleep = async () => {};

describe("retryingFetch", () => {
  test("returns the first successful response without retrying", async () => {
    let calls = 0;
    const wrapped = retryingFetch(
      async () => {
        calls++;
        return new Response("ok", { status: 200 });
      },
      { sleep: noSleep },
    );

    const res = await wrapped("https://api.github.com/rate_limit");
    expect(res.status).toBe(200);
    expect(calls).toBe(1);
  });

  test("retries 5xx up to the retry budget and returns the last response", async () => {
    let calls = 0;
    const wrapped = retryingFetch(
      async () => {
        calls++;
        return new Response("boom", { status: 503 });
      },
      { retries: 2, sleep: noSleep },
    );

    const res = await wrapped("https://api.github.com/rate_limit");
    expect(res.status).toBe(503);
    expect(calls).toBe(3);
  });

  test("recovers when a retry succeeds", async () => {
    let calls = 0;
    const wrapped = retryingFetch(
      async () => {
        calls++;
        return calls < 3 ? new Response("", { status: 502 }) : new Response("ok", { status: 200 });
      },
      { retries: 2, sleep: noSleep },
    );

    expect((await wrapped("https://api.github.com/x")).status).toBe(200);
    expect(calls).toBe(3);
  });

  test("does not retry 4xx", async () => {
    let calls = 0;
    const wrapped = retryingFetch(
      async () => {
        calls++;
        return new Response("nope", { status: 404 });
      },
      { retries: 2, sleep: noSleep },
    );

    expect((await wrapped("https://api.github.com/x")).status).toBe(404);
    expect(calls).toBe(1);
  });

  test("retries network errors then rethrows the last one", async () => {
    let calls = 0;
    const wrapped = retryingFetch(
      async () => {
        calls++;
        throw new Error(`down ${calls}`);
      },
      { retries: 2, sleep: noSleep },
    );

    await expect(wrapped("https://api.github.com/x")).rejects.toThrow("down 3");
    expect(calls).toBe(3);
  });

  test("passes an abort signal so a hung request cannot run forever", async () => {
    const wrapped = retryingFetch(
      (_input, init) =>
        new Promise<Response>((_resolve, reject) => {
          init?.signal?.addEventListener("abort", () => reject(new Error("aborted")));
        }),
      { retries: 0, timeoutMs: 5, sleep: noSleep },
    );

    await expect(wrapped("https://api.github.com/x")).rejects.toThrow("aborted");
  });

  test("a caller abort is not retried", async () => {
    const controller = new AbortController();
    let calls = 0;
    const wrapped = retryingFetch(
      async (_input, init) => {
        calls++;
        init?.signal?.throwIfAborted();
        throw new Error("unreachable");
      },
      { retries: 2, sleep: noSleep },
    );

    controller.abort();
    await expect(
      wrapped("https://api.github.com/x", { signal: controller.signal }),
    ).rejects.toThrow();
    expect(calls).toBe(1);
  });

  test("exposes a bounded timeout, retry budget, and identifying user-agent", () => {
    expect(REQUEST_TIMEOUT_MS).toBeGreaterThan(0);
    expect(REQUEST_TIMEOUT_MS).toBeLessThanOrEqual(60_000);
    expect(REQUEST_RETRIES).toBeGreaterThan(0);
    expect(USER_AGENT).toMatch(/^cctui-ghreview\/\d/);
  });
});
