const clock = new Intl.DateTimeFormat(undefined, { hour: '2-digit', minute: '2-digit', hourCycle: 'h23' });
const full = new Intl.DateTimeFormat(undefined, { dateStyle: 'medium', timeStyle: 'medium' });

/** "09:48", 24-hour whatever the locale. */
export const shortTime = (ts: number): string => clock.format(ts);
export const fullTime = (ts: number): string => full.format(ts);
export const isoTime = (ts: number): string => new Date(ts).toISOString();
