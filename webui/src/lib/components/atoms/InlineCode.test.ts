import { mount, unmount } from "svelte";
import { afterEach, describe, expect, it } from "vitest";
import InlineCode, { SLOT } from "./InlineCode.svelte";

let target: HTMLElement | null = null;
let app: Record<string, unknown> | null = null;

afterEach(() => {
	if (app) unmount(app);
	target?.remove();
	app = null;
	target = null;
});

const render = (text: string, code: string) => {
	target = document.createElement("div");
	document.body.appendChild(target);
	app = mount(InlineCode, { target, props: { text, code } });
	return target;
};

describe("InlineCode", () => {
	it("renders the text either side of the slot with the code between them", () => {
		const el = render(`Install ${SLOT} on the target machine.`, "cctui-daemon");
		expect(el.textContent).toBe("Install cctui-daemon on the target machine.");
	});

	it("lets the message put the code first", () => {
		const el = render(`${SLOT} must be set in the config.`, "ghreviewUrl");
		expect(el.textContent).toBe("ghreviewUrl must be set in the config.");
	});

	it("lets the message put the code last", () => {
		const el = render(`Passed to the worker as ${SLOT}`, "--name");
		expect(el.textContent).toBe("Passed to the worker as --name");
	});

	it("renders a message with no slot verbatim and no code span", () => {
		const el = render("Nothing to interpolate.", "unused");
		expect(el.textContent).toBe("Nothing to interpolate.");
		expect(el.textContent).not.toContain("unused");
	});
});
