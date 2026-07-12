export type Route =
  | { name: "home" }
  | { name: "inbox" }
  | { name: "bookmarklet" }
  | { name: "pull"; owner: string; repo: string; number: number }
  | { name: "notfound"; path: string };

const PULL_RE = /^\/([^/]+)\/([^/]+)\/pull\/(\d+)\/?$/;

export function parseRoute(pathname: string): Route {
  const path = pathname || "/";
  if (path === "/" || path === "") return { name: "home" };
  if (path === "/inbox" || path === "/inbox/") return { name: "inbox" };
  if (path === "/bookmarklet" || path === "/bookmarklet/") return { name: "bookmarklet" };
  const m = PULL_RE.exec(path);
  if (m) {
    return { name: "pull", owner: m[1], repo: m[2], number: Number(m[3]) };
  }
  return { name: "notfound", path };
}

export function pullPath(owner: string, repo: string, number: number): string {
  return `/${owner}/${repo}/pull/${number}`;
}

const PULL_API_RE = /\/repos\/([^/]+)\/([^/]+)\/pulls\/(\d+)(?:$|[/?#])/;

export function parsePullApiUrl(
  url: string | null | undefined,
): { owner: string; repo: string; number: number } | null {
  if (!url) return null;
  const m = PULL_API_RE.exec(url);
  if (!m) return null;
  return { owner: m[1], repo: m[2], number: Number(m[3]) };
}
