export interface TemplateExpressions {
	recognised: string[];
	unrecognised: string[];
}

export const DEFAULT_TEMPLATE = '{{event}} {{payload.action}}: {{payload.issue.title}}';

export const isRecognised = (expr: string): boolean =>
	expr === 'event' || expr === 'delivery' || expr === 'payload' || expr.startsWith('payload.');

export function templateExpressions(template: string): TemplateExpressions {
	const recognised = new Set<string>();
	const unrecognised = new Set<string>();
	let rest = template;
	for (;;) {
		const open = rest.indexOf('{{');
		if (open < 0) break;
		const len = rest.slice(open + 2).indexOf('}}');
		if (len < 0) break;
		const expr = rest.slice(open + 2, open + 2 + len).trim();
		(isRecognised(expr) ? recognised : unrecognised).add(expr);
		rest = rest.slice(open + len + 4);
	}
	return { recognised: [...recognised], unrecognised: [...unrecognised] };
}
