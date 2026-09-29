import { mount, tick, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { router } from "../router/router.svelte";
import { tabs } from "../stores/tabs.svelte";
import TabBar from "./TabBar.svelte";

let component: ReturnType<typeof mount> | undefined;

function tablist(): HTMLElement {
  return document.querySelector('[role="tablist"]') as HTMLElement;
}

function tabEls(): HTMLElement[] {
  return [...document.querySelectorAll('[role="tab"]')] as HTMLElement[];
}

function selectedTab(): HTMLElement | undefined {
  return tabEls().find((t) => t.getAttribute("aria-selected") === "true");
}

function press(key: string): void {
  tablist().dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true }));
}

async function render(): Promise<void> {
  component = mount(TabBar, { target: document.body });
  await tick();
}

beforeEach(() => {
  localStorage.clear();
  for (const tab of [...tabs.tabs]) tabs.close(tab.id);
  router.navigate("/", true);
  tabs.open("acme", "web", 11, "Eleven");
  tabs.open("acme", "web", 22, "Twenty two");
  tabs.open("acme", "api", 33, "Thirty three");
});

afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  document.body.replaceChildren();
  for (const tab of [...tabs.tabs]) tabs.close(tab.id);
  localStorage.clear();
  router.navigate("/", true);
});

describe("TabBar", () => {
  it("renders the open pull requests as a labelled tablist", async () => {
    await render();

    expect(document.querySelector("nav.tabbar")).not.toBeNull();
    expect(tablist().getAttribute("aria-label")).toBe("Open pull requests");
    expect(tabEls()).toHaveLength(3);
    expect(tabEls().map((t) => (t.textContent ?? "").replace(/\s+/g, " ").trim())).toEqual([
      "#11 Eleven",
      "#22 Twenty two",
      "#33 Thirty three",
    ]);
  });

  it("keeps exactly one tab in the Tab order", async () => {
    router.navigate("/acme/web/pull/22");
    await render();

    expect(tabEls().map((t) => t.getAttribute("tabindex"))).toEqual(["-1", "0", "-1"]);
  });

  it("moves selection with the arrow keys and navigates to the tab", async () => {
    router.navigate("/acme/web/pull/22");
    await render();

    press("ArrowRight");
    await tick();
    expect(router.current).toMatchObject({ name: "pull", repo: "api", number: 33 });
    expect((selectedTab()?.textContent ?? "").trim()).toContain("#33");

    press("ArrowLeft");
    await tick();
    expect(router.current).toMatchObject({ name: "pull", repo: "web", number: 22 });
  });

  it("wraps with the arrow keys and jumps with Home and End", async () => {
    router.navigate("/acme/api/pull/33");
    await render();

    press("ArrowRight");
    await tick();
    expect(router.current).toMatchObject({ name: "pull", number: 11 });

    press("End");
    await tick();
    expect(router.current).toMatchObject({ name: "pull", number: 33 });

    press("Home");
    await tick();
    expect(router.current).toMatchObject({ name: "pull", number: 11 });
  });

  it("activates a tab on click", async () => {
    await render();

    tabEls()[0].click();
    await tick();
    expect(router.current).toMatchObject({ name: "pull", repo: "web", number: 11 });
    expect(tabs.activeId).toBe("pr-acme-web-11");
  });

  it("closes a tab through its close control", async () => {
    router.navigate("/acme/web/pull/11");
    await render();

    const close = tabEls()[1].querySelector("button") as HTMLButtonElement;
    close.click();
    await tick();

    expect(tabs.tabs.map((t) => t.number)).toEqual([11, 33]);
    expect(router.current).toMatchObject({ name: "pull", number: 11 });
  });

  it("closes every tab and returns to the root", async () => {
    router.navigate("/acme/web/pull/11");
    await render();

    const closeAll = document.querySelector(
      'button[aria-label="Close all tabs"]',
    ) as HTMLButtonElement;
    closeAll.click();
    await tick();

    expect(tabs.tabs).toHaveLength(0);
    expect(router.current.name).toBe("root");
  });

  it("hides the close-all action while a single tab is open", async () => {
    for (const tab of [...tabs.tabs].slice(1)) tabs.close(tab.id);
    await render();

    expect(tabs.tabs).toHaveLength(1);
    expect(document.querySelector('button[aria-label="Close all tabs"]')).toBeNull();
  });
});
