import { parseRoute, type Route } from "./route";

class Router {
  current = $state<Route>(parseRoute(window.location.pathname));

  constructor() {
    window.addEventListener("popstate", () => {
      this.current = parseRoute(window.location.pathname);
    });
  }

  navigate(path: string, replace = false): void {
    if (replace) window.history.replaceState({}, "", path);
    else window.history.pushState({}, "", path);
    this.current = parseRoute(path);
  }
}

export const router = new Router();
