import { basePath } from "../api/config";
import { parseRoute, type Route } from "./route";

// parseRoute only understands GitHub-mirrored app-relative paths; strip/add the
// embed base path (CCT-610) at the history boundary so it stays that way.
function toAppPath(fullPath: string): string {
  const bp = basePath();
  if (!bp) return fullPath;
  if (fullPath === bp) return "/";
  if (fullPath.startsWith(`${bp}/`)) return fullPath.slice(bp.length);
  return fullPath;
}

function toFullPath(appPath: string): string {
  const bp = basePath();
  if (!bp) return appPath;
  return appPath === "/" ? bp : `${bp}${appPath}`;
}

class Router {
  current = $state<Route>(parseRoute(toAppPath(window.location.pathname)));

  constructor() {
    window.addEventListener("popstate", () => this.refresh());
  }

  // The singleton constructs at import time, before an embedder can set the base
  // path; Review calls this after configureRuntime to re-derive the route.
  refresh(): void {
    this.current = parseRoute(toAppPath(window.location.pathname));
  }

  navigate(path: string, replace = false): void {
    const full = toFullPath(path);
    if (replace) window.history.replaceState({}, "", full);
    else window.history.pushState({}, "", full);
    this.current = parseRoute(path);
  }
}

export const router = new Router();
