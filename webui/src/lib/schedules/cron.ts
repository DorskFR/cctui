export type RepeatKind = 'hourly' | 'daily' | 'weekdays' | 'weekly' | 'custom';

export const REPEAT_KINDS: readonly RepeatKind[] = ['hourly', 'daily', 'weekdays', 'weekly', 'custom'];

export interface Repeat {
	kind: RepeatKind;
	time: string;
	weekdays: number[];
	custom: string;
}

export type CronFieldName = 'minute' | 'hour' | 'day_of_month' | 'month' | 'day_of_week';

export type CronProblem =
	| { kind: 'empty' }
	| { kind: 'fields'; count: number }
	| { kind: 'field'; field: CronFieldName; value: string; min: number; max: number };

const FIELDS: readonly { name: CronFieldName; min: number; max: number }[] = [
	{ name: 'minute', min: 0, max: 59 },
	{ name: 'hour', min: 0, max: 23 },
	{ name: 'day_of_month', min: 1, max: 31 },
	{ name: 'month', min: 1, max: 12 },
	{ name: 'day_of_week', min: 0, max: 7 }
];

const DEFAULT_TIME = '09:00';

const split = (expr: string): string[] => expr.trim().split(/\s+/).filter(Boolean);

const int = (raw: string): number | null => (/^\d{1,2}$/.test(raw) ? Number(raw) : null);

function itemOk(raw: string, min: number, max: number): boolean {
	const [range, step, ...extra] = raw.split('/');
	if (extra.length) return false;
	if (step !== undefined) {
		const n = int(step);
		if (n === null || n < 1) return false;
	}
	if (range === '*') return true;
	const bounds = range.split('-');
	if (bounds.length > 2) return false;
	const nums = bounds.map(int);
	if (nums.some((n) => n === null || n < min || n > max)) return false;
	return nums.length === 1 || (nums[0] as number) <= (nums[1] as number);
}

export function cronProblem(expr: string): CronProblem | null {
	const parts = split(expr);
	if (parts.length === 0) return { kind: 'empty' };
	if (parts.length !== FIELDS.length) return { kind: 'fields', count: parts.length };
	for (const [i, { name, min, max }] of FIELDS.entries()) {
		const value = parts[i];
		if (!value.split(',').every((item) => itemOk(item, min, max))) {
			return { kind: 'field', field: name, value, min, max };
		}
	}
	return null;
}

export function parseTime(time: string): { hour: number; minute: number } | null {
	const hit = /^(\d{1,2}):(\d{2})$/.exec(time.trim());
	if (!hit) return null;
	const hour = Number(hit[1]);
	const minute = Number(hit[2]);
	return hour <= 23 && minute <= 59 ? { hour, minute } : null;
}

export const formatTime = (hour: number, minute: number): string =>
	`${String(hour).padStart(2, '0')}:${String(minute).padStart(2, '0')}`;

export function normalizeWeekdays(days: readonly number[]): number[] {
	return [...new Set(days.map((d) => d % 7))].sort((a, b) => a - b);
}

export function cronFromRepeat(repeat: Repeat): string {
	if (repeat.kind === 'custom') return split(repeat.custom).join(' ');
	if (repeat.kind === 'hourly') return '0 * * * *';
	const at = parseTime(repeat.time) ?? { hour: 9, minute: 0 };
	const head = `${at.minute} ${at.hour} * *`;
	if (repeat.kind === 'daily') return `${head} *`;
	if (repeat.kind === 'weekdays') return `${head} 1-5`;
	const days = normalizeWeekdays(repeat.weekdays);
	return `${head} ${(days.length ? days : [1]).join(',')}`;
}

export type CronShape =
	| { kind: 'every_minute' }
	| { kind: 'every_minutes'; step: number }
	| { kind: 'hourly'; minute: number }
	| { kind: 'every_hours'; step: number }
	| { kind: 'daily'; hour: number; minute: number }
	| { kind: 'weekdays'; hour: number; minute: number }
	| { kind: 'weekly'; hour: number; minute: number; days: number[] }
	| { kind: 'monthly'; hour: number; minute: number; day: number }
	| { kind: 'other' };

const stepOf = (field: string): number | null => {
	const hit = /^\*\/(\d{1,2})$/.exec(field);
	return hit ? Number(hit[1]) : null;
};

function weekdayList(field: string): number[] | null {
	const days = field.split(',').map(int);
	return days.every((d) => d !== null) ? normalizeWeekdays(days as number[]) : null;
}

export function cronShape(expr: string): CronShape {
	if (cronProblem(expr)) return { kind: 'other' };
	const [min, hour, dom, month, dow] = split(expr);
	if (month !== '*') return { kind: 'other' };
	const minute = int(min);
	const anyDay = dom === '*' && dow === '*';
	if (hour === '*') {
		if (!anyDay) return { kind: 'other' };
		if (min === '*') return { kind: 'every_minute' };
		const step = stepOf(min);
		if (step !== null) return step === 1 ? { kind: 'every_minute' } : { kind: 'every_minutes', step };
		return minute === null ? { kind: 'other' } : { kind: 'hourly', minute };
	}
	if (minute === null) return { kind: 'other' };
	const hourStep = stepOf(hour);
	if (hourStep !== null) {
		if (!anyDay || minute !== 0) return { kind: 'other' };
		return hourStep === 1 ? { kind: 'hourly', minute } : { kind: 'every_hours', step: hourStep };
	}
	const h = int(hour);
	if (h === null) return { kind: 'other' };
	if (anyDay) return { kind: 'daily', hour: h, minute };
	if (dow === '*') {
		const day = int(dom);
		return day === null ? { kind: 'other' } : { kind: 'monthly', hour: h, minute, day };
	}
	if (dom !== '*') return { kind: 'other' };
	if (dow === '1-5') return { kind: 'weekdays', hour: h, minute };
	const days = weekdayList(dow);
	if (!days) return { kind: 'other' };
	if (days.length === 7) return { kind: 'daily', hour: h, minute };
	return { kind: 'weekly', hour: h, minute, days };
}

export function repeatFromCron(cron: string | null | undefined): Repeat {
	const base: Repeat = { kind: 'daily', time: DEFAULT_TIME, weekdays: [1], custom: '' };
	if (!cron?.trim()) return base;
	const custom = split(cron).join(' ');
	const shape = cronShape(cron);
	switch (shape.kind) {
		case 'hourly':
			return shape.minute === 0 ? { ...base, kind: 'hourly', custom } : { ...base, kind: 'custom', custom };
		case 'daily':
		case 'weekdays':
			return { ...base, kind: shape.kind, time: formatTime(shape.hour, shape.minute), custom };
		case 'weekly':
			return {
				...base,
				kind: 'weekly',
				time: formatTime(shape.hour, shape.minute),
				weekdays: shape.days,
				custom
			};
		default:
			return { ...base, kind: 'custom', custom };
	}
}
