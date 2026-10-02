// @vitest-environment happy-dom
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

describe("settings resync when the tab comes back", () => {
  let get: ReturnType<typeof vi.spyOn>;
  const server = (sort: string) => ({ version: 1, data: { sessionList: { sort } } });

  beforeEach(async () => {
    get = vi.spyOn(api, "get").mockResolvedValue(server("name"));
    await settings.load();
    settings.flush();
    await vi.runAllTimersAsync();
    expect(settings.saveStatus).not.toBe("pending");
  });

  afterEach(() => get.mockRestore());

  it("re-reads what another tab saved before this one can write it back", async () => {
    get.mockResolvedValue(server("created"));
    window.dispatchEvent(new Event("focus"));
    await vi.waitFor(() => expect(settings.state.sessionList.sort).toBe("created"));
    expect(put).not.toHaveBeenCalled();
  });

  it("keeps this tab's pending write instead of re-reading over it", async () => {
    settings.setSessionList({ sort: "name" });
    get.mockClear();
    window.dispatchEvent(new Event("focus"));
    expect(get).not.toHaveBeenCalled();
  });

  it("drops a re-read that lands after a local edit", async () => {
    let resolve: (v: unknown) => void = () => {};
    get.mockReturnValue(new Promise((r) => (resolve = r)));
    window.dispatchEvent(new Event("focus"));
    expect(get).toHaveBeenCalled();
    settings.setSessionList({ sort: "name" });
    resolve(server("created"));
    await vi.advanceTimersByTimeAsync(0);
    expect(settings.state.sessionList.sort).toBe("name");
  });
});

