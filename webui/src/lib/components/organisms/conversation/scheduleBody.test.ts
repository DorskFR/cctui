import { describe, expect, it, vi } from 'vitest';
import { scheduleBody } from './scheduleBody';

describe('scheduleBody', () => {
	it('uploads the files before scheduling the body that names them', async () => {
		const order: string[] = [];
		const stage = vi.fn(async (t: string) => {
			order.push('stage');
			return `${t}\n\nAttached file:\n- /tmp/cctui-uploads/s/a.png`;
		});
		const schedule = vi.fn(async () => {
			order.push('schedule');
		});
		const out = await scheduleBody('look [a.png]', stage, schedule);
		expect(order).toEqual(['stage', 'schedule']);
		expect(schedule).toHaveBeenCalledWith('look [a.png]\n\nAttached file:\n- /tmp/cctui-uploads/s/a.png');
		expect(out).toEqual({ ok: true, body: 'look [a.png]\n\nAttached file:\n- /tmp/cctui-uploads/s/a.png' });
	});

	it('schedules nothing when the upload failed', async () => {
		const schedule = vi.fn();
		expect(await scheduleBody('x', async () => null, schedule)).toEqual({ ok: false, restore: null });
		expect(schedule).not.toHaveBeenCalled();
	});

	it('hands back the staged body when scheduling fails', async () => {
		const err = new Error('down');
		const out = await scheduleBody('x', async () => 'x\n\nAttached file:\n- /p', async () => {
			throw err;
		});
		expect(out).toEqual({ ok: false, error: err, restore: 'x\n\nAttached file:\n- /p' });
	});
});
