import { describe, expect, it } from 'vitest';
import {
	cronFromRepeat,
	cronProblem,
	cronShape,
	normalizeWeekdays,
	parseTime,
	repeatFromCron,
	type Repeat
} from './cron';
import { cronProblemText, cronWords, weekdayName } from './cronWords';

const repeat = (over: Partial<Repeat>): Repeat => ({
	kind: 'daily',
	time: '09:00',
	weekdays: [1],
	custom: '',
	...over
});

describe('cronFromRepeat', () => {
	it('generates the cron of every preset', () => {
		expect(cronFromRepeat(repeat({ kind: 'hourly' }))).toBe('0 * * * *');
		expect(cronFromRepeat(repeat({ kind: 'daily', time: '09:00' }))).toBe('0 9 * * *');
		expect(cronFromRepeat(repeat({ kind: 'weekdays', time: '07:00' }))).toBe('0 7 * * 1-5');
		expect(cronFromRepeat(repeat({ kind: 'weekly', time: '18:30', weekdays: [5, 1] }))).toBe('30 18 * * 1,5');
		expect(cronFromRepeat(repeat({ kind: 'custom', custom: '  */15  8-18 * * * ' }))).toBe('*/15 8-18 * * *');
	});

	it('falls back to Monday when a weekly repeat has no day', () => {
		expect(cronFromRepeat(repeat({ kind: 'weekly', weekdays: [] }))).toBe('0 9 * * 1');
	});
});

describe('repeatFromCron', () => {
	it('recognises a generated cron back into its preset', () => {
		expect(repeatFromCron('0 * * * *').kind).toBe('hourly');
		expect(repeatFromCron('0 9 * * *')).toMatchObject({ kind: 'daily', time: '09:00' });
		expect(repeatFromCron('0 7 * * 1-5')).toMatchObject({ kind: 'weekdays', time: '07:00' });
		expect(repeatFromCron('30 18 * * 1,5')).toMatchObject({ kind: 'weekly', time: '18:30', weekdays: [1, 5] });
	});

	it('reads day 7 as Sunday', () => {
		expect(repeatFromCron('5 4 * * 7')).toMatchObject({ kind: 'weekly', time: '04:05', weekdays: [0] });
	});

	it('keeps anything else as a custom expression', () => {
		for (const cron of ['15 * * * *', '*/15 8-18 * * *', '0 9 1 * *', '0 9 * 6 *', '0 9 * * 1-3']) {
			expect(repeatFromCron(cron)).toMatchObject({ kind: 'custom', custom: cron });
		}
	});

	it('round-trips every preset through its cron', () => {
		for (const cron of ['0 * * * *', '45 23 * * *', '0 7 * * 1-5', '0 9 * * 0,6']) {
			expect(cronFromRepeat(repeatFromCron(cron))).toBe(cron);
		}
	});

	it('starts a new schedule on a daily repeat', () => {
		expect(repeatFromCron(null)).toMatchObject({ kind: 'daily', time: '09:00' });
	});
});

describe('cronProblem', () => {
	it('accepts the shapes the server accepts', () => {
		for (const cron of ['* * * * *', '0 9 * * 1-5', '*/15 8-18 * * *', '0,30 6 1 1,6 0', '5 4 * * 7']) {
			expect(cronProblem(cron)).toBeNull();
		}
	});

	it('names what is wrong', () => {
		expect(cronProblem('  ')).toEqual({ kind: 'empty' });
		expect(cronProblem('0 9 * *')).toEqual({ kind: 'fields', count: 4 });
		expect(cronProblem('@daily')).toEqual({ kind: 'fields', count: 1 });
		expect(cronProblem('60 9 * * *')).toMatchObject({ kind: 'field', field: 'minute', value: '60' });
		expect(cronProblem('0 24 * * *')).toMatchObject({ field: 'hour' });
		expect(cronProblem('0 9 0 * *')).toMatchObject({ field: 'day_of_month' });
		expect(cronProblem('0 9 * 13 *')).toMatchObject({ field: 'month' });
		expect(cronProblem('0 9 * * mon')).toMatchObject({ field: 'day_of_week', value: 'mon' });
		expect(cronProblem('*/0 * * * *')).toMatchObject({ field: 'minute' });
		expect(cronProblem('5-1 * * * *')).toMatchObject({ field: 'minute' });
	});

	it('words the problem for the reader', () => {
		expect(cronProblemText({ kind: 'fields', count: 4 })).toContain('five fields');
		const text = cronProblemText({ kind: 'field', field: 'hour', value: '24', min: 0, max: 23 });
		expect(text).toContain('hour');
		expect(text).toContain('"24"');
		expect(text).toContain('0 to 23');
	});
});

describe('cronWords', () => {
	it('puts the common shapes in words', () => {
		expect(cronWords('0 * * * *', 'en')).toBe('Every hour');
		expect(cronWords('15 * * * *', 'en')).toBe('Every hour at :15');
		expect(cronWords('* * * * *', 'en')).toBe('Every minute');
		expect(cronWords('*/15 * * * *', 'en')).toBe('Every 15 minutes');
		expect(cronWords('0 */2 * * *', 'en')).toBe('Every 2 hours');
		expect(cronWords('0 9 * * *', 'en')).toBe('Every day at 09:00');
		expect(cronWords('0 7 * * 1-5', 'en')).toBe('Weekdays at 07:00');
		expect(cronWords('0 9 * * 1', 'en')).toBe('Every Monday at 09:00');
		expect(cronWords('30 18 * * 1,5', 'en')).toBe('Every Monday and Friday at 18:30');
		expect(cronWords('0 9 1 * *', 'en')).toBe('Day 1 of every month at 09:00');
	});

	it('falls back to the raw expression', () => {
		expect(cronWords('*/15 8-18 * * *', 'en')).toBe('*/15 8-18 * * *');
		expect(cronWords('not a cron', 'en')).toBe('not a cron');
	});

	it('names weekdays from Sunday as day 0', () => {
		expect(weekdayName(0, 'en')).toBe('Sunday');
		expect(weekdayName(7, 'en')).toBe('Sunday');
		expect(weekdayName(1, 'en', 'short')).toBe('Mon');
	});
});

describe('helpers', () => {
	it('parses a time of day', () => {
		expect(parseTime('07:05')).toEqual({ hour: 7, minute: 5 });
		expect(parseTime('24:00')).toBeNull();
		expect(parseTime('')).toBeNull();
	});

	it('sorts and dedupes weekdays, folding 7 into 0', () => {
		expect(normalizeWeekdays([5, 7, 1, 5, 0])).toEqual([0, 1, 5]);
	});

	it('reads every day of the week as daily', () => {
		expect(cronShape('0 9 * * 0,1,2,3,4,5,6')).toEqual({ kind: 'daily', hour: 9, minute: 0 });
	});
});
