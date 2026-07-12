// See https://svelte.dev/docs/kit/types#app.d.ts
declare global {
	namespace App {
		// interface Error {}
		// interface Locals {}
		// interface PageData {}
		// interface PageState {}
		// interface Platform {}
	}

	interface Window {
		CCTUI_CONFIG?: {
			apiBase?: string;
			ghreviewUrl?: string;
		};
	}

	/** Injected at build time by vite (`define`); "dev" for local builds. */
	const __CLIENT_VERSION__: string;
}

export {};
