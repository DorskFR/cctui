import { basePath } from "../api/config";
import { parseRoute, type Route } from "./route";

// parseRoute only understands GitHub-mirrored app-relative paths; strip/add the
// embed base path at the history boundary so it stays that way.
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

export interface RouterHost {
  navigate(path: string): void;
}

class Router {
  current = $state<Route>(parseRoute(toAppPath(window.location.pathname)));
  #host: RouterHost | null = null;

  constructor() {
    window.addEventListener("popstate", () => this.refresh());
  }

  // A plugin page does not own the address bar: the host routes, hands the
  // sub-path down as a prop and takes navigations back through `navigate`.
  adopt(host: RouterHost | null): void {
    this.#host = host;
  }

  setPath(appPath: string): void {
    this.current = parseRoute(appPath || "/");
  }

  // The singleton constructs at import time, before an embedder can set the base
  // path; Review calls this after configureRuntime to re-derive the route.
  refresh(): void {
    if (this.#host) return;
    this.current = parseRoute(toAppPath(window.location.pathname));
  }

  navigate(path: string, replace = false): void {
    if (this.#host) {
      this.current = parseRoute(path);
      this.#host.navigate(path);
      return;
    }
    const full = toFullPath(path);
    if (replace) window.history.replaceState({}, "", full);
    else window.history.pushState({}, "", full);
    this.current = parseRoute(path);
  }
}

export const router = new Router();
