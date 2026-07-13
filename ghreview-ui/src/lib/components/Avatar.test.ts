import { mount, unmount } from "svelte";
import { afterEach, describe, expect, it } from "vitest";
import Avatar, { initialOf, sizedAvatarUrl } from "./Avatar.svelte";

let component: ReturnType<typeof mount> | undefined;

afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  document.body.replaceChildren();
});

describe("initialOf", () => {
  it("returns the uppercased first letter of the login", () => {
    expect(initialOf("octocat")).toBe("O");
    expect(initialOf("  dorsk")).toBe("D");
  });

  it("falls back to a neutral placeholder when login is missing", () => {
    expect(initialOf(undefined)).toBe("?");
    expect(initialOf(null)).toBe("?");
    expect(initialOf("")).toBe("?");
  });
});

describe("sizedAvatarUrl", () => {
  it("appends a retina size hint to a bare avatar url", () => {
    expect(sizedAvatarUrl("https://avatars.githubusercontent.com/u/1", 20)).toBe(
      "https://avatars.githubusercontent.com/u/1?s=40",
    );
  });

  it("overrides an existing size query param", () => {
    expect(sizedAvatarUrl("https://example.com/a?s=460&v=4", 16)).toBe(
      "https://example.com/a?s=32&v=4",
    );
  });

  it("returns null when no url is provided", () => {
    expect(sizedAvatarUrl(undefined, 20)).toBeNull();
    expect(sizedAvatarUrl(null, 20)).toBeNull();
  });
});

describe("Avatar", () => {
  it("renders the sized image when avatar_url is present", () => {
    component = mount(Avatar, {
      target: document.body,
      props: { user: { login: "octocat", avatar_url: "https://example.com/a" }, size: 20 },
    });
    const img = document.querySelector("img") as HTMLImageElement | null;
    expect(img).not.toBeNull();
    expect(img?.getAttribute("src")).toBe("https://example.com/a?s=40");
    expect(document.querySelector(".initial")).toBeNull();
  });

  it("falls back to the initial letter when avatar_url is missing", () => {
    component = mount(Avatar, {
      target: document.body,
      props: { user: { login: "octocat" }, size: 20 },
    });
    expect(document.querySelector("img")).toBeNull();
    expect(document.querySelector(".initial")?.textContent).toBe("O");
  });

  it("falls back to a neutral placeholder when the user is absent", () => {
    component = mount(Avatar, {
      target: document.body,
      props: { user: null },
    });
    expect(document.querySelector("img")).toBeNull();
    expect(document.querySelector(".initial")?.textContent).toBe("?");
  });
});
