import { expect, test } from '@playwright/test';
import { stubDrawer, waitForDrawerOpen } from './drawer.fixture';
import { watch } from './tiles.fixture';

test.use({
	permissions: ['microphone'],
	launchOptions: { args: ['--use-fake-device-for-media-stream', '--use-fake-ui-for-media-stream'] }
});

test.skip(({ browserName }) => browserName !== 'chromium', 'fake media devices are Chromium flags');

test('live voice initialises the VAD without an error toast', async ({ page }) => {
	const errors = watch(page);
	await stubDrawer(page);
	await page.route('**/api/v1/voice/config', (route) =>
		route.fulfill({
			json: {
				enabled: true,
				stt: true,
				tts: true,
				stt_model: 'parakeet',
				stt_language: null,
				tts_model: 'kokoro',
				tts_voice: 'af_heart',
				tts_format: 'opus'
			}
		})
	);
	await page.routeWebSocket(/\/ws\/voice/, (ws) => ws.onMessage(() => {}));

	await page.setViewportSize({ width: 1536, height: 900 });
	await page.goto('/sessions');
	await page.locator('[data-journey="session"] [data-journey="title"]').first().click();
	await waitForDrawerOpen(page);

	const ortModule = page.waitForResponse((r) => r.url().endsWith('/vad/ort-wasm-simd-threaded.mjs'));
	await page.getByRole('button', { name: 'Live voice conversation with this session' }).click();

	const mjs = await ortModule;
	expect(mjs.status()).toBe(200);
	expect(mjs.headers()['content-type']).toMatch(/^(text|application)\/javascript/);

	await expect(page.locator('[data-journey="voice-live"]')).toContainText('Listening', { timeout: 30_000 });
	await expect(page.locator('.toaster-host .toast')).toHaveCount(0);
	expect(errors).toEqual([]);
});
