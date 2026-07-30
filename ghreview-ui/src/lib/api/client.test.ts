import { afterEach, describe, expect, it, vi } from "vitest";
import { api, collectCursorPages } from "./client";
import { configureRuntime } from "./config";

afterEach(() => {
  configureRuntime(null);
  vi.restoreAllMocks();
});

describe("api client with injected runtime auth", () => {
  it("prefixes the injected baseUrl and sends the injected bearer token", async () => {
    configureRuntime({ baseUrl: "https://ghreview.example", token: "session-token" });
    const fetchMock = vi.fn(
      async () =>
        new Response(JSON.stringify({ ok: true }), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        }),
    );
    vi.stubGlobal("fetch", fetchMock);

    await api.status();

    const [url, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe("https://ghreview.example/v1/status");
    const headers = new Headers(init.headers);
    expect(headers.get("Authorization")).toBe("Bearer session-token");
  });

  it("omits the Authorization header when the injected token is null", async () => {
    configureRuntime({ baseUrl: "https://ghreview.example", token: null });
    const fetchMock = vi.fn(
      async () => new Response(null, { status: 204 }),
    );
    vi.stubGlobal("fetch", fetchMock);

    await api.status();

    const [, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit];
    const headers = new Headers(init.headers);
    expect(headers.get("Authorization")).toBeNull();
  });
});

describe("path segment encoding", () => {
  function jsonFetch() {
    const fetchMock = vi.fn(
      async (_input: string | URL | Request, _init?: RequestInit) =>
        new Response(JSON.stringify({}), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        }),
    );
    vi.stubGlobal("fetch", fetchMock);
    return fetchMock;
  }

  it("encodes owner, repo and number segments", async () => {
    configureRuntime({ baseUrl: "https://ghreview.example", token: null });
    const fetchMock = jsonFetch();

    await api.pull("Org Name", "repo#weird/name", 7);

    expect(String(fetchMock.mock.calls[0][0])).toBe(
      "https://ghreview.example/v1/repos/Org%20Name/repo%23weird%2Fname/pulls/7",
    );
  });

  it("encodes label and comment id segments", async () => {
    configureRuntime({ baseUrl: "https://ghreview.example", token: null });
    const fetchMock = jsonFetch();

    await api.removePullLabel("o", "r", 1, "acct", "needs review/triage");
    await api.deleteIssueComment("o", "r", 99, "acct");
    await api.unsubscribe("o/r#7");

    const urls = fetchMock.mock.calls.map(([url]) => String(url));
    expect(urls[0]).toBe(
      "https://ghreview.example/v1/repos/o/r/pulls/1/labels/needs%20review%2Ftriage?account=acct",
    );
    expect(urls[1]).toBe(
      "https://ghreview.example/v1/repos/o/r/issues/comments/99?account=acct",
    );
    expect(urls[2]).toBe("https://ghreview.example/v1/subscriptions/o%2Fr%237");
  });

  it("keeps the pulls listing path stable for ordinary names", async () => {
    configureRuntime({ baseUrl: "https://ghreview.example", token: null });
    const fetchMock = jsonFetch();

    await api.pulls("octo", "one", "octocat");

    expect(String(fetchMock.mock.calls[0][0])).toBe(
      "https://ghreview.example/v1/repos/octo/one/pulls?account=octocat",
    );
  });
});

describe("cursor pagination", () => {
  it("collects every page in cursor order", async () => {
    const fetchPage = vi
      .fn()
      .mockResolvedValueOnce({ items: [1, 2], next_cursor: "page-2" })
      .mockResolvedValueOnce({ items: [3], next_cursor: "page-3" })
      .mockResolvedValueOnce({ items: [4, 5], next_cursor: null });

    await expect(collectCursorPages(fetchPage)).resolves.toEqual([1, 2, 3, 4, 5]);
    expect(fetchPage.mock.calls).toEqual([[undefined], ["page-2"], ["page-3"]]);
  });

  it("collects all repository and pull pages through the API helpers", async () => {
    configureRuntime({ baseUrl: "https://ghreview.example", token: null });
    const fetchMock = vi.fn(async (input: string | URL | Request) => {
      const url = new URL(String(input));
      const cursor = url.searchParams.get("cursor");
      const isPullPage = url.pathname.endsWith("/pulls");
      const body = isPullPage
        ? cursor
          ? { items: [{ kind: "pull_request", payload: { number: 32 } }], next_cursor: null }
          : {
              items: [{ kind: "pull_request", payload: { number: 1 } }],
              next_cursor: "pull-page-2",
            }
        : cursor
          ? { items: [{ kind: "repo", payload: { full_name: "octo/two" } }], next_cursor: null }
          : {
              items: [{ kind: "repo", payload: { full_name: "octo/one" } }],
              next_cursor: "repo-page-2",
            };
      return new Response(JSON.stringify(body), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      });
    });
    vi.stubGlobal("fetch", fetchMock);

    const repos = await api.allRepos("octocat");
    const pulls = await api.allPulls("octo", "one", "octocat");

    expect(repos).toHaveLength(2);
    expect(pulls).toHaveLength(2);
    expect(fetchMock.mock.calls.map(([url]) => String(url))).toEqual([
      "https://ghreview.example/v1/repos?account=octocat&limit=100",
      "https://ghreview.example/v1/repos?account=octocat&limit=100&cursor=repo-page-2",
      "https://ghreview.example/v1/repos/octo/one/pulls?account=octocat&limit=100",
      "https://ghreview.example/v1/repos/octo/one/pulls?account=octocat&limit=100&cursor=pull-page-2",
    ]);
  });
});
