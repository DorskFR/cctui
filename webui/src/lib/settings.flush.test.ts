import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { settings } from "./settings.svelte";
import { auth } from "./auth.svelte";
import { api } from "./api";

let put: ReturnType<typeof vi.spyOn>;

beforeEach(() => {
  vi.useFakeTimers();
  localStorage.clear();
  auth.isAuthed = true;
  put = vi.spyOn(api, "put").mockResolvedValue(undefined);
});

afterEach(() => {
  put.mockRestore();
  vi.useRealTimers();
  auth.isAuthed = false;
  localStorage.clear();
});

describe("settings debounced write", () => {
  it("flushes a pending PUT on pagehide instead of losing it to the reload", () => {
    settings.setSessionList({ sort: "name" });
    expect(put).not.toHaveBeenCalled();

    window.dispatchEvent(new Event("pagehide"));

    expect(put).toHaveBeenCalledTimes(1);
    const [path, body, opts] = put.mock.calls[0] as [
      string,
      { data: { sessionList: { sort: string } } },
      { keepalive?: boolean } | undefined,
    ];
    expect(path).toBe("/settings");
    expect(body.data.sessionList.sort).toBe("name");
    expect(opts?.keepalive).toBe(true);

    vi.advanceTimersByTime(1000);
    expect(put).toHaveBeenCalledTimes(1);
  });

  it("flushes on visibilitychange to hidden", () => {
    settings.setSessionList({ sort: "created" });
    vi.spyOn(document, "visibilityState", "get").mockReturnValue("hidden");

    document.dispatchEvent(new Event("visibilitychange"));

    expect(put).toHaveBeenCalledTimes(1);
  });

  it("is a no-op when nothing is pending", () => {
    window.dispatchEvent(new Event("pagehide"));
    expect(put).not.toHaveBeenCalled();
  });

  it("still coalesces a burst of writes into one debounced PUT", () => {
    settings.setSessionList({ sort: "name" });
    vi.advanceTimersByTime(200);
    settings.setSessionList({ sort: "created" });
    vi.advanceTimersByTime(200);
    expect(put).not.toHaveBeenCalled();

    vi.advanceTimersByTime(200);
    expect(put).toHaveBeenCalledTimes(1);
    const [, body] = put.mock.calls[0] as [
      string,
      { data: { sessionList: { sort: string } } },
    ];
    expect(body.data.sessionList.sort).toBe("created");
  });
});
