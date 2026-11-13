import type { Labels } from '@dorsk/hmm';
import { m } from '$lib/paraglide/messages';

/** Never add a user identifier, token or session content: reports leave this deployment. */
export function feedbackContext(serverVersion: string | undefined, route: string | null): Record<string, string> {
	const context: Record<string, string> = { webuiVersion: __CLIENT_VERSION__ };
	if (serverVersion) context.serverVersion = serverVersion;
	if (route) context.route = route;
	return context;
}

export const feedbackLabels = (): Labels => ({
	launcher: m.feedback_launcher(),
	title: m.feedback_title(),
	subtitle: m.feedback_subtitle(),
	close: m.feedback_close(),
	details: m.feedback_details(),
	commentPlaceholder: m.feedback_comment_placeholder(),
	attach: m.feedback_attach(),
	pick: m.feedback_pick(),
	shot: m.feedback_shot(),
	pickHint: m.feedback_pick_hint(),
	capturing: m.feedback_capturing(),
	itemPlaceholder: m.feedback_item_placeholder(),
	screenshot: m.feedback_screenshot(),
	remove: m.feedback_remove(),
	removeImage: m.feedback_remove_image(),
	reviewHint: m.feedback_review_hint(),
	discard: m.feedback_discard(),
	review: m.feedback_review(),
	back: m.feedback_back(),
	send: m.feedback_send(),
	reviewIntro: m.feedback_review_intro(),
	yourFeedback: m.feedback_your_feedback(),
	page: m.feedback_page(),
	comment: m.feedback_comment(),
	technical: m.feedback_technical(),
	sent: m.feedback_sent(),
	queued: m.feedback_queued(),
	failed: m.feedback_failed(),
	blocked: m.feedback_blocked(),
	shotFailed: m.feedback_shot_failed(),
	full: m.feedback_full(),
	done: m.feedback_done()
});
