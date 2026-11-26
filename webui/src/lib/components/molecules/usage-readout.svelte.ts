import { readoutColumns } from './cap-bar.logic';

const slots = new Set<UsageReadout>();
let observer: ResizeObserver | null = null;
let frame = 0;

function realign() {
	frame = 0;
	const list = [...slots].flatMap((s) => (s.root && s.readout ? [{ s, root: s.root, readout: s.readout }] : []));
	const rem = Number.parseFloat(getComputedStyle(document.documentElement).fontSize) || 16;
	const widths = readoutColumns(
		list.map(({ root, readout }) => ({
			group: root.closest('[data-usage-group]') ?? root.parentElement,
			right: root.getBoundingClientRect().right,
			width: readout.getBoundingClientRect().width
		}))
	);
	list.forEach(({ s }, i) => {
		s.px = widths[i];
		s.rem = rem;
	});
}

/** One usage row's readout, sized to the widest readout among the rows stacked with it
 *  inside the closest `[data-usage-group]` (else the row's parent). */
export class UsageReadout {
	px = $state(0);
	rem = $state(16);
	root: HTMLElement | null = null;
	readout: HTMLElement | null = null;

	track(root: HTMLElement, readout: HTMLElement): () => void {
		if (typeof ResizeObserver === 'undefined') return () => {};
		this.root = root;
		this.readout = readout;
		slots.add(this);
		observer ??= new ResizeObserver(realign);
		observer.observe(root);
		observer.observe(readout);
		return () => {
			observer?.unobserve(root);
			observer?.unobserve(readout);
			slots.delete(this);
			this.root = this.readout = null;
			if (slots.size && !frame) frame = requestAnimationFrame(realign);
		};
	}
}
