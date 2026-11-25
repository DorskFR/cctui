import { m } from '#lib/paraglide/messages.js';
import { cronShape, formatTime, type CronFieldName, type CronProblem } from './cron';

const SUNDAY_UTC = Date.UTC(2023, 0, 1);

export function weekdayName(day: number, locale: string, width: 'long' | 'short' = 'long'): string {
	return new Intl.DateTimeFormat(locale, { weekday: width, timeZone: 'UTC' }).format(
		new Date(SUNDAY_UTC + (day % 7) * 86_400_000)
	);
}

export function cronWords(cron: string, locale: string): string {
	const shape = cronShape(cron);
	switch (shape.kind) {
		case 'every_minute':
			return m.schedules_words_every_minute();
		case 'every_minutes':
			return m.schedules_words_every_minutes({ count: shape.step });
		case 'hourly':
			return shape.minute === 0
				? m.schedules_words_hourly()
				: m.schedules_words_hourly_at({ minute: String(shape.minute).padStart(2, '0') });
		case 'every_hours':
			return m.schedules_words_every_hours({ count: shape.step });
		case 'daily':
			return m.schedules_words_daily({ time: formatTime(shape.hour, shape.minute) });
		case 'weekdays':
			return m.schedules_words_weekdays({ time: formatTime(shape.hour, shape.minute) });
		case 'weekly':
			return m.schedules_words_weekly({
				days: new Intl.ListFormat(locale, { type: 'conjunction' }).format(
					shape.days.map((d) => weekdayName(d, locale))
				),
				time: formatTime(shape.hour, shape.minute)
			});
		case 'monthly':
			return m.schedules_words_monthly({
				day: shape.day,
				time: formatTime(shape.hour, shape.minute)
			});
		default:
			return cron.trim();
	}
}

const fieldLabel: Record<CronFieldName, () => string> = {
	minute: m.schedules_cron_field_minute,
	hour: m.schedules_cron_field_hour,
	day_of_month: m.schedules_cron_field_day_of_month,
	month: m.schedules_cron_field_month,
	day_of_week: m.schedules_cron_field_day_of_week
};

export function cronProblemText(problem: CronProblem): string {
	if (problem.kind === 'empty') return m.schedules_cron_error_empty();
	if (problem.kind === 'fields') return m.schedules_cron_error_fields({ count: problem.count });
	return m.schedules_cron_error_field({
		field: fieldLabel[problem.field](),
		value: problem.value,
		min: problem.min,
		max: problem.max
	});
}
