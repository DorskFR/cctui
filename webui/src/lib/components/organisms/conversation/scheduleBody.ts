export type ScheduleOutcome =
	| { ok: true; body: string }
	| { ok: false; error?: unknown; restore: string | null };

/** Upload the attachments, then schedule the body that names them. After a
 *  failed schedule `restore` holds that body: the files are already staged, so
 *  the composer keeps their paths instead of uploading again. */
export async function scheduleBody(
	text: string,
	stage: (text: string) => Promise<string | null>,
	schedule: (body: string) => Promise<unknown>
): Promise<ScheduleOutcome> {
	const body = await stage(text);
	if (body === null) return { ok: false, restore: null };
	try {
		await schedule(body);
		return { ok: true, body };
	} catch (error) {
		return { ok: false, error, restore: body };
	}
}
