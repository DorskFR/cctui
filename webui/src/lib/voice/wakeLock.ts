/** Keeps the phone's screen on while the voice loop runs; a no-op where the
 *  Screen Wake Lock API is missing. Returns the release function. */
export async function acquireWakeLock(): Promise<() => void> {
	const nav = typeof navigator === 'undefined' ? undefined : navigator;
	const api = nav?.wakeLock;
	if (!api) return () => {};
	let sentinel: WakeLockSentinel | null = null;
	try {
		sentinel = await api.request('screen');
	} catch {
		return () => {};
	}
	const reacquire = () => {
		if (document.visibilityState === 'visible' && sentinel?.released) {
			void api.request('screen').then((s) => (sentinel = s)).catch(() => {});
		}
	};
	document.addEventListener('visibilitychange', reacquire);
	return () => {
		document.removeEventListener('visibilitychange', reacquire);
		void sentinel?.release().catch(() => {});
		sentinel = null;
	};
}
