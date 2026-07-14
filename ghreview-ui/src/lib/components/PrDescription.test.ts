import { mount, unmount } from "svelte";
import { afterEach, describe, expect, it } from "vitest";
import PrDescription from "./PrDescription.svelte";

let component: ReturnType<typeof mount> | undefined;

afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  document.body.replaceChildren();
});

describe("PrDescription", () => {
  it("renders semantic Markdown with canonical repository link context", () => {
    component = mount(PrDescription, {
      target: document.body,
      props: {
        owner: "example",
        repo: "project",
        body: "## Details\n\n- One\n- Two\n\n[Guide](docs/guide.md)",
      },
    });

    expect(document.querySelector(".prdesc h2")?.textContent).toBe("Details");
    expect(document.querySelectorAll(".prdesc li")).toHaveLength(2);
    expect(document.querySelector<HTMLAnchorElement>(".prdesc a")?.href).toBe(
      "https://github.com/example/project/docs/guide.md",
    );
  });

  it("embeds a safe GitHub attachment without relaying raw attributes", () => {
    component = mount(PrDescription, {
      target: document.body,
      props: {
        owner: "example",
        repo: "project",
        body: '<img src="/user-attachments/assets/01234567-89ab-cdef-0123-456789abcdef" alt="Screenshot" onerror="alert(1)" />',
      },
    });

    const image = document.querySelector<HTMLImageElement>(".prdesc img");
    expect(image?.src).toBe(
      "https://github.com/user-attachments/assets/01234567-89ab-cdef-0123-456789abcdef",
    );
    expect(image?.alt).toBe("Screenshot");
    expect(image?.loading).toBe("lazy");
    expect(image?.getAttribute("decoding")).toBe("async");
    expect(image?.getAttribute("referrerpolicy")).toBe("no-referrer");
    expect(image?.hasAttribute("onerror")).toBe(false);
  });
});
