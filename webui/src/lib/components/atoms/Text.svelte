<script lang="ts">
	// Text primitive: the ONLY place body/label/caption/code text and its bearing
	// elements (<p>/<span>/<label>/<div> of pure text) are emitted. `variant` picks
	// a token preset; `tone`/`weight`/`size` override individual axes; bare <Text>
	// (no props) inherits the surrounding style like a plain <span> so it can wrap
	// inline glue text without changing rendering. All values come from tokens.
	import type { Snippet } from 'svelte';

	type Size = 'xs' | 'sm' | 'base' | 'md' | 'lg' | 'xl' | '2xl';

	let {
		as = 'span',
		variant,
		tone = 'inherit',
		weight,
		size,
		truncate = false,
		class: klass = '',
		children,
		...rest
	}: {
		as?: 'span' | 'p' | 'div' | 'label';
		// body: default reading text · label: form-label · caption: small meta ·
		// code: monospace. Omit for inline glue that should inherit its parent.
		variant?: 'body' | 'label' | 'caption' | 'code';
		tone?: 'inherit' | 'default' | 'muted' | 'faint' | 'danger' | 'accent';
		weight?: 'normal' | 'medium' | 'semibold' | 'bold';
		size?: Size;
		truncate?: boolean;
		class?: string;
		children?: Snippet;
		[key: string]: unknown;
	} = $props();
</script>

<svelte:element
	this={as}
	class="text {variant ? `v-${variant}` : ''} tone-{tone} {weight ? `fw-${weight}` : ''} {size
		? `fs-${size}`
		: ''} {truncate ? 'truncate' : ''} {klass}"
	{...rest}
>
	{@render children?.()}
</svelte:element>

<style>
	/* Base: inherit everything — a bare <Text> renders like a plain span. */
	.text {
		margin: 0;
	}
	/* Variants (presets) — listed before tone/weight/size so those override. */
	.v-body {
		font-size: var(--fs-base);
		line-height: var(--lh-normal);
		color: var(--text);
	}
	.v-label {
		font-size: var(--fs-sm);
		font-weight: var(--fw-medium);
		color: var(--text-muted);
	}
	.v-caption {
		font-size: var(--fs-xs);
		color: var(--text-faint);
	}
	.v-code {
		font-family: var(--font-mono);
		font-size: 0.92em;
	}
	/* Tone (colour) — overrides variant colour. */
	.tone-default {
		color: var(--text);
	}
	.tone-muted {
		color: var(--text-muted);
	}
	.tone-faint {
		color: var(--text-faint);
	}
	.tone-danger {
		color: var(--danger);
	}
	.tone-accent {
		color: var(--accent);
	}
	/* Weight — overrides variant weight. */
	.fw-normal {
		font-weight: var(--fw-normal);
	}
	.fw-medium {
		font-weight: var(--fw-medium);
	}
	.fw-semibold {
		font-weight: var(--fw-semibold);
	}
	.fw-bold {
		font-weight: var(--fw-bold);
	}
	/* Size — overrides variant size (listed last so it wins). */
	.fs-xs {
		font-size: var(--fs-xs);
	}
	.fs-sm {
		font-size: var(--fs-sm);
	}
	.fs-base {
		font-size: var(--fs-base);
	}
	.fs-md {
		font-size: var(--fs-md);
	}
	.fs-lg {
		font-size: var(--fs-lg);
	}
	.fs-xl {
		font-size: var(--fs-xl);
	}
	.fs-2xl {
		font-size: var(--fs-2xl);
	}
	.truncate {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		min-width: 0;
	}
</style>
