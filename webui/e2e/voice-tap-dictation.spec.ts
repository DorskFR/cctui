import { expect, test } from '@playwright/test';
import { stubDrawer, waitForDrawerOpen } from './drawer.fixture';
import { watch } from './tiles.fixture';

const MOBILE = { width: 390, height: 844 };

test.use({
	permissions: ['microphone'],
	launchOptions: { args: ['--use-fake-device-for-media-stream', '--use-fake-ui-for-media-stream'] }
});

test.skip(({ browserName }) => browserName !== 'chromium', 'fake media devices are Chromium flags');

test('390px: dictation starts with one tap, shows a large stop target, and stops with another', async ({
	page
}) => {
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
	await page.route('**/api/v1/voice/transcribe', (route) =>
		route.fulfill({ json: { text: 'hello from the microphone' } })
	);

	await page.setViewportSize(MOBILE);
	await page.goto('/sessions');
	await page.locator('[data-journey="session"] [data-journey="title"]').first().click();
	await waitForDrawerOpen(page);

	const mic = page.getByRole('button', { name: /^Dictate into the message box/ });
	await mic.click();

	const recording = page.locator('[data-journey="voice-recording"]');
	await expect(recording).toBeVisible();
	await expect(mic).toHaveAttribute('aria-pressed', 'true');
	const stop = recording.getByRole('button', { name: 'Stop and send' });
	const box = await stop.boundingBox();
	expect(box).not.toBeNull();
	expect(box!.height).toBeGreaterThanOrEqual(48);
	expect(box!.width).toBeGreaterThanOrEqual(200);
	expect(box!.x + box!.width).toBeLessThanOrEqual(MOBILE.width);

	await page.waitForTimeout(1500);
	await expect(recording).toContainText('0:01');
	await stop.click();

	await expect(recording).toHaveCount(0);
	await expect(page.locator('[data-journey="composer"] textarea')).toHaveValue(/hello from the microphone/, {
		timeout: 10_000
	});
	await expect(page.locator('.toaster-host .toast')).toHaveCount(0);
	expect(errors).toEqual([]);
});
