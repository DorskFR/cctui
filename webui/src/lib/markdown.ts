/**
 * Safe-ish markdown -> HTML for chat text bubbles (CCT-161). Everything is
 * escaped first, so no raw HTML from the model survives; we then re-introduce a
 * small, fixed set of tags. All colors are CSS-variable driven (see
 * `--md-*` / `--syn-*` in variables.css) so themes adapt.
 *
 * Adds over the legacy renderer:
 *  - per-language fenced-code highlighting (lang from the info-string)
 *  - Claude-terminal feel: grayish prose, bold = bright, `inline code` = blue
 *  - headings, lists, blockquotes
 *  - leaked `<system message>` / harness pseudo-tags rendered as muted markup
 *    instead of being dropped or shown as broken text.
 */

export function escapeHtml(s: string): string {
	return s
		.replace(/&/g, '&amp;')
		.replace(/</g, '&lt;')
		.replace(/>/g, '&gt;')
		.replace(/"/g, '&quot;');
}

// Terminal output (diffs, tool stdout) often carries ANSI escape sequences. They
// aren't HTML-escaped by escapeHtml, so left in place they leak into the DOM as
// raw control bytes — visible as garbled `28→29`-style artifacts that turn
// pink/red when copied into a terminal. Strip the SGR/CSI/OSC sequences and any
// stray C0 control chars (keeping \t and \n) before rendering. The ANSI pattern
// is the well-worn `ansi-regex` one (ESC / CSI introducers + parameter bytes).
// eslint-disable-next-line no-control-regex
const ANSI_RE =
	/[\x1B\x9B][[\]()#;?]*(?:(?:(?:(?:;[-a-zA-Z\d/#&.:=?%@~_]+)*|[a-zA-Z\d]+(?:;[-a-zA-Z\d/#&.:=?%@~_]*)*)?\x07)|(?:(?:\d{1,4}(?:;\d{0,4})*)?[\dA-PR-TZcf-ntqry=><~]))/g;
// C0 control chars except tab (\x09) and newline (\x0A), plus DEL (\x7F).
// eslint-disable-next-line no-control-regex
const C0_RE = /[\x00-\x08\x0B-\x1F\x7F]/g;

export function stripAnsi(s: string): string {
	return s.replace(ANSI_RE, '').replace(C0_RE, '');
}

// Sentinels for placeholder protection — characters that never appear in source
// text or in our escaped HTML, so restore passes can't collide with content.
const SLOT_L = '';
const SLOT_R = '';
const BLOCK_L = '';
const BLOCK_R = '';

// Harness / system pseudo-tags that sometimes leak into model text as literal
// markup. We render them as a muted inline chip rather than dropping them.
const PSEUDO_TAG =
	/&lt;(\/?(?:system[- ]message|system-reminder|task-notification|command-name|command-message|local-command[^&]*|bash-input|bash-stdout|bash-stderr)[^&]*?)&gt;/gi;

// ── Syntax highlighting ─────────────────────────────────────────────────────
// Lightweight, regex-based, token-class -> CSS-variable. Not a full lexer; aims
// for "good enough" Claude-Code-terminal feel without pulling in a heavy dep.

const KEYWORDS: Record<string, string> = {
	js: 'const|let|var|function|return|if|else|for|while|class|new|import|export|from|async|await|try|catch|finally|throw|typeof|instanceof|extends|super|this|null|undefined|true|false|switch|case|break|continue|default|of|in|yield|do|delete|void',
	ts: 'const|let|var|function|return|if|else|for|while|class|new|import|export|from|async|await|try|catch|finally|throw|typeof|instanceof|extends|super|this|null|undefined|true|false|switch|case|break|continue|default|of|in|yield|interface|type|enum|implements|public|private|protected|readonly|as|keyof|namespace|declare|abstract',
	py: 'def|return|if|elif|else|for|while|class|import|from|as|try|except|finally|raise|with|lambda|yield|async|await|None|True|False|and|or|not|in|is|pass|break|continue|global|nonlocal|assert|del|self',
	rust: 'fn|let|mut|const|return|if|else|for|while|loop|match|struct|enum|impl|trait|pub|use|mod|crate|self|super|async|await|move|ref|where|dyn|as|in|Some|None|Ok|Err|true|false|unsafe|type',
	go: 'func|return|if|else|for|range|switch|case|default|var|const|type|struct|interface|map|chan|go|defer|package|import|nil|true|false|break|continue|select|fallthrough',
	sh: 'if|then|else|elif|fi|for|while|do|done|case|esac|function|return|in|export|local|echo|cd|set'
};
const LANG_ALIAS: Record<string, string> = {
	javascript: 'js',
	jsx: 'js',
	mjs: 'js',
	typescript: 'ts',
	tsx: 'ts',
	python: 'py',
	rs: 'rust',
	golang: 'go',
	bash: 'sh',
	shell: 'sh',
	zsh: 'sh'
};

function highlightCode(rawCode: string, lang: string): string {
	const norm = LANG_ALIAS[lang.toLowerCase()] ?? lang.toLowerCase();
	if (norm === 'json') return highlightJson(rawCode);

	const kw = KEYWORDS[norm];
	// Work against escaped text so we never emit unescaped markup.
	let s = escapeHtml(stripAnsi(rawCode));

	// Placeholder protection for strings/comments so later passes don't touch them.
	const slots: string[] = [];
	const stash = (html: string) => {
		const i = slots.push(html) - 1;
		return `${SLOT_L}${i}${SLOT_R}`;
	};

	// comments (/* block */, // line, # line)
	s = s.replace(/\/\*[\s\S]*?\*\//g, (m) => stash(`<span class="syn-comment">${m}</span>`));
	s = s.replace(/(^|[^:])\/\/[^\n]*/g, (m, p) => p + stash(`<span class="syn-comment">${m.slice(p.length)}</span>`));
	if (norm === 'py' || norm === 'sh' || norm === 'rust' || norm === 'go' || !kw) {
		s = s.replace(/#[^\n]*/g, (m) => stash(`<span class="syn-comment">${m}</span>`));
	}
	// strings (", ', `) — escaped double-quotes are &quot;
	s = s.replace(/(&quot;|['`])(?:\\.|(?!\1)[\s\S])*?\1/g, (m) => stash(`<span class="syn-string">${m}</span>`));

	// numbers
	s = s.replace(/\b(0x[0-9a-fA-F]+|\d+\.?\d*(?:[eE][+-]?\d+)?)\b/g, '<span class="syn-number">$1</span>');

	// keywords
	if (kw) {
		const re = new RegExp(`\\b(${kw})\\b`, 'g');
		s = s.replace(re, '<span class="syn-keyword">$1</span>');
	}
	// function-call names: ident immediately before "("
	s = s.replace(/\b([A-Za-z_]\w*)(\s*\()/g, '<span class="syn-function">$1</span>$2');

	// restore stashed comment/string slots
	s = s.replace(new RegExp(`${SLOT_L}(\\d+)${SLOT_R}`, 'g'), (_m, i) => slots[Number(i)]);
	return s;
}

function highlightJson(raw: string): string {
	let s = escapeHtml(stripAnsi(raw));
	// keys "..." :
	s = s.replace(/(&quot;(?:\\.|[^&]|&(?!quot;))*?&quot;)(\s*:)/g, '<span class="syn-function">$1</span>$2');
	// remaining strings
	s = s.replace(/(&quot;(?:\\.|[^&]|&(?!quot;))*?&quot;)/g, '<span class="syn-string">$1</span>');
	// numbers
	s = s.replace(/\b(-?\d+\.?\d*(?:[eE][+-]?\d+)?)\b/g, '<span class="syn-number">$1</span>');
	// literals
	s = s.replace(/\b(true|false|null)\b/g, '<span class="syn-keyword">$1</span>');
	return s;
}

// ── Markdown ────────────────────────────────────────────────────────────────

export function renderMarkdown(src: string): string {
	// Strip terminal control sequences before any structural parsing.
	src = stripAnsi(src);
	// Protect fenced code blocks before escaping the rest.
	const blocks: string[] = [];
	let s = src.replace(/```([^\n`]*)\n?([\s\S]*?)```/g, (_m, info: string, code: string) => {
		const lang = (info || '').trim().split(/\s+/)[0] ?? '';
		const body = highlightCode(code.replace(/\n$/, ''), lang);
		const cls = lang ? ` data-lang="${escapeHtml(lang)}"` : '';
		const i = blocks.push(`<pre class="md-pre"${cls}><code>${body}</code></pre>`) - 1;
		return `${BLOCK_L}${i}${BLOCK_R}`;
	});

	s = escapeHtml(s);

	// Leaked harness pseudo-tags -> muted chip (don't show as broken text).
	s = s.replace(PSEUDO_TAG, '<span class="md-meta-tag">&lt;$1&gt;</span>');

	// inline code
	s = s.replace(/`([^`]+)`/g, '<code class="md-code">$1</code>');
	// bold
	s = s.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');
	// italic (avoid touching ** already consumed)
	s = s.replace(/(^|[^*])\*([^*\n]+)\*/g, '$1<em>$2</em>');
	// links [text](url)
	s = s.replace(
		/\[([^\]]+)\]\((https?:\/\/[^\s)]+)\)/g,
		'<a href="$2" target="_blank" rel="noopener noreferrer">$1</a>'
	);
	// headings -> styled bold line
	s = s.replace(/^#{1,6}\s+(.+)$/gm, '<span class="md-h">$1</span>');
	// blockquote
	s = s.replace(/^&gt;\s?(.*)$/gm, '<span class="md-quote">$1</span>');
	// unordered list items
	s = s.replace(/^\s*[-*]\s+(.+)$/gm, '<span class="md-li">• $1</span>');
	// line breaks
	s = s.replace(/\n/g, '<br />');

	// restore code blocks
	s = s.replace(new RegExp(`${BLOCK_L}(\\d+)${BLOCK_R}`, 'g'), (_m, i) => blocks[Number(i)]);
	return s;
}

/** Highlight a standalone code/JSON string for a <pre> bubble (tool calls,
 * results). Returns escaped, span-wrapped HTML for use with {@html}. */
export function highlightBlock(raw: string, lang = ''): string {
	return highlightCode(raw, lang);
}

export function prettyJson(v: unknown): string {
	try {
		return JSON.stringify(v, null, 2);
	} catch {
		return String(v);
	}
}
