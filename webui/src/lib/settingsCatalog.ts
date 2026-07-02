// Per-account Claude Code settings catalog — GENERATED-FROM-CATALOG.
//
// This is a hand-transcribed mirror of the server-side source of truth
// `crates/cctui-server/src/settings_catalog/catalog.toml` (CCT-537). There is no
// HTTP catalog endpoint (the server module is embedded, not exposed), so the
// webui account settings editor (CCT-541) carries its own typed copy of the
// SAFE/CARE settings keys, the curated env-var allowlist, and the "Quiet
// defaults" preset. Keep in sync with catalog.toml when it changes.
//
// Only SAFE and CARE keys are ever surfaced to the operator — MANAGED and SYSTEM
// keys are deliberately omitted here, mirroring `Policy::account_exposable()`
// server-side. The server re-validates every write against the real catalog, so
// this copy is a UX affordance, not the security boundary.

/** Exposure policy — only `safe`/`care` are settable per-account. */
export type Policy = 'safe' | 'care';

/** How the editor renders a settings key. Only `bool` keys become toggles; the
 *  rest are documented and edited through the raw-JSON box. */
export type SettingKind = 'bool' | 'enum' | 'string' | 'other';

export interface CatalogKey {
	/** settings.json top-level key name. */
	name: string;
	/** Policy tag (safe = low blast radius, care = has caveats). */
	tag: Policy;
	/** Render kind — the editor only builds a control for `bool`. */
	kind: SettingKind;
	/** UI grouping for the toggle list. */
	group: string;
	/** Human-readable label (defaults to the key name). */
	label: string;
	/** Documented default, as a display string. */
	default?: string;
	/** Short description / caveat. */
	notes: string;
}

// Only the boolean SAFE/CARE keys are enumerated here (they drive the toggle
// list). Non-boolean SAFE/CARE keys (enums like editorMode / preferredNotifChannel,
// strings, arrays, objects) are still settable via the raw-JSON box below and are
// validated server-side — they're just not given a bespoke control.
export const CATALOG_KEYS: CatalogKey[] = [
	// --- Skills & workflows ---
	{ name: 'disableBundledSkills', tag: 'safe', kind: 'bool', group: 'Skills & workflows', label: 'Disable bundled skills', notes: 'Disable bundled skills/workflows; built-ins hidden but typable.' },
	{ name: 'disableWorkflows', tag: 'safe', kind: 'bool', group: 'Skills & workflows', label: 'Disable workflows', default: 'false', notes: 'Disable dynamic workflows + bundled workflow commands.' },
	{ name: 'disableSkillShellExecution', tag: 'safe', kind: 'bool', group: 'Skills & workflows', label: 'Disable skill shell execution', notes: 'Disable inline !… shell in skills/commands.' },
	{ name: 'disableArtifact', tag: 'safe', kind: 'bool', group: 'Skills & workflows', label: 'Disable Artifact tool', notes: 'Turn off the Artifact tool.' },
	{ name: 'enableArtifact', tag: 'safe', kind: 'bool', group: 'Skills & workflows', label: 'Enable Artifact tool', default: 'availability', notes: 'Enable the Artifact tool per user.' },
	{ name: 'disableAgentView', tag: 'safe', kind: 'bool', group: 'Skills & workflows', label: 'Disable background agents', notes: 'Turn off background agents / agent view.' },

	// --- Remote control (the ONLY per-account/device remote toggle) ---
	{ name: 'disableRemoteControl', tag: 'care', kind: 'bool', group: 'Remote control', label: 'Disable Remote Control', notes: 'Disable Remote Control for this account/device (v2.1.128+). This is the only remote toggle that is per-device/per-account.' },
	{ name: 'remoteControlAtStartup', tag: 'care', kind: 'bool', group: 'Remote control', label: 'Connect Remote Control at startup', default: 'org default', notes: 'Auto-connect Remote Control on launch.' },

	// --- Notifications ---
	{ name: 'agentPushNotifEnabled', tag: 'safe', kind: 'bool', group: 'Notifications', label: 'Agent push notifications', default: 'false', notes: 'Proactive phone push.' },
	{ name: 'inputNeededNotifEnabled', tag: 'safe', kind: 'bool', group: 'Notifications', label: 'Input-needed push', default: 'false', notes: 'Phone push on a permission/question.' },
	{ name: 'awaySummaryEnabled', tag: 'safe', kind: 'bool', group: 'Notifications', label: 'Away summary', notes: 'Session recap when returning.' },

	// --- Privacy & telemetry ---
	{ name: 'autoMemoryEnabled', tag: 'safe', kind: 'bool', group: 'Privacy & memory', label: 'Auto memory', notes: 'Auto memory read/write to .claude/memory/.' },

	// --- UI & transcript ---
	{ name: 'alwaysThinkingEnabled', tag: 'safe', kind: 'bool', group: 'UI & transcript', label: 'Always extended thinking', notes: 'Extended thinking on by default.' },
	{ name: 'showThinkingSummaries', tag: 'safe', kind: 'bool', group: 'UI & transcript', label: 'Show thinking summaries', notes: 'Show thinking summaries in the transcript (Ctrl+O).' },
	{ name: 'showTurnDuration', tag: 'safe', kind: 'bool', group: 'UI & transcript', label: 'Show turn duration', notes: 'Show "Cooked for 1m 6s".' },
	{ name: 'showClearContextOnPlanAccept', tag: 'safe', kind: 'bool', group: 'UI & transcript', label: 'Offer clear-context on plan accept', notes: 'Offer "clear context" on plan approval.' },
	{ name: 'spinnerTipsEnabled', tag: 'safe', kind: 'bool', group: 'UI & transcript', label: 'Spinner tips', notes: 'Show spinner tips.' },
	{ name: 'terminalProgressBarEnabled', tag: 'safe', kind: 'bool', group: 'UI & transcript', label: 'Terminal progress bar', notes: 'Show the terminal progress bar.' },
	{ name: 'prefersReducedMotion', tag: 'safe', kind: 'bool', group: 'UI & transcript', label: 'Reduce motion', notes: 'Reduce UI animations.' },
	{ name: 'autoScrollEnabled', tag: 'safe', kind: 'bool', group: 'UI & transcript', label: 'Auto-scroll', default: 'true', notes: 'Follow output to the bottom (fullscreen).' },
	{ name: 'axScreenReader', tag: 'safe', kind: 'bool', group: 'UI & transcript', label: 'Screen-reader output', notes: 'Screen-reader flat output.' },

	// --- Editing & safety ---
	{ name: 'autoCompactEnabled', tag: 'safe', kind: 'bool', group: 'Editing & safety', label: 'Auto-compact', default: 'true', notes: 'Auto-compact near the context limit.' },
	{ name: 'fileCheckpointingEnabled', tag: 'safe', kind: 'bool', group: 'Editing & safety', label: 'File checkpointing', default: 'true', notes: 'Snapshot files before edits for /rewind.' },
	{ name: 'useAutoModeDuringPlan', tag: 'safe', kind: 'bool', group: 'Editing & safety', label: 'Auto-mode during plan', notes: 'Auto-approve safe reads during plan mode.' },
	{ name: 'respectGitignore', tag: 'safe', kind: 'bool', group: 'Editing & safety', label: 'Respect .gitignore', notes: 'The @ picker respects .gitignore.' },
	{ name: 'includeGitInstructions', tag: 'safe', kind: 'bool', group: 'Editing & safety', label: 'Include git instructions', notes: 'Git commit/PR instructions + status in the system prompt.' },
	{ name: 'disableClaudeAiConnectors', tag: 'care', kind: 'bool', group: 'Editing & safety', label: 'Disable claude.ai connectors', notes: 'Disable claude.ai MCP connectors.' },
];

/** A curated env var exposed as an account default. */
export interface CatalogEnv {
	name: string;
	group: string;
	tag: Policy;
	notes: string;
}

// Mirrors the `[[env]]` allowlist in catalog.toml (the curated subset — NOT all
// documented vars). The editor restricts the extra-env editor to these names;
// the server re-validates against the real allowlist.
export const CATALOG_ENV: CatalogEnv[] = [
	{ name: 'ANTHROPIC_MODEL', group: 'model', tag: 'safe', notes: 'Primary model id; overridden by --model / /model.' },
	{ name: 'CLAUDE_CODE_SUBAGENT_MODEL', group: 'model', tag: 'safe', notes: 'Model for subagents; `inherit` == unset.' },
	{ name: 'CLAUDE_CODE_EFFORT_LEVEL', group: 'model', tag: 'safe', notes: 'Reasoning effort low/medium/high/xhigh/max/auto.' },
	{ name: 'CLAUDE_CODE_DISABLE_1M_CONTEXT', group: 'context', tag: 'safe', notes: 'Disable 1M context; Sonnet treated as 200K (=1).' },
	{ name: 'CLAUDE_CODE_MAX_CONTEXT_TOKENS', group: 'context', tag: 'care', notes: 'Override assumed context window (tokens).' },
	{ name: 'CLAUDE_CODE_AUTO_COMPACT_WINDOW', group: 'context', tag: 'care', notes: 'Context capacity for auto-compaction (tokens).' },
	{ name: 'CLAUDE_AUTOCOMPACT_PCT_OVERRIDE', group: 'context', tag: 'care', notes: '%-of-window compaction trigger, lowers only (1-100).' },
	{ name: 'MAX_THINKING_TOKENS', group: 'thinking', tag: 'safe', notes: 'Extended-thinking budget; 0 disables on the Anthropic API.' },
	{ name: 'CLAUDE_CODE_DISABLE_THINKING', group: 'thinking', tag: 'care', notes: 'Omit the thinking param, proxy compat (=1).' },
	{ name: 'CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING', group: 'thinking', tag: 'care', notes: 'Fixed MAX_THINKING_TOKENS instead of adaptive (=1).' },
	{ name: 'DISABLE_INTERLEAVED_THINKING', group: 'thinking', tag: 'care', notes: 'Drop the interleaved-thinking beta header (=1).' },
	{ name: 'CLAUDE_CODE_MAX_OUTPUT_TOKENS', group: 'tokens', tag: 'care', notes: 'Max output tokens/request (reduces effective context).' },
	{ name: 'CLAUDE_CODE_FILE_READ_MAX_OUTPUT_TOKENS', group: 'tokens', tag: 'safe', notes: 'File-read token cap.' },
	{ name: 'MAX_MCP_OUTPUT_TOKENS', group: 'tokens', tag: 'safe', notes: 'Max tokens in MCP tool responses (default 25000).' },
	{ name: 'TASK_MAX_OUTPUT_LENGTH', group: 'tokens', tag: 'safe', notes: 'Subagent output char cap (default 32000, max 160000).' },
	{ name: 'BASH_MAX_OUTPUT_LENGTH', group: 'tokens', tag: 'safe', notes: 'Bash output char cap before file spill.' },
	{ name: 'CLAUDE_CODE_DISABLE_BUNDLED_SKILLS', group: 'skills', tag: 'safe', notes: 'Disable bundled skills/workflows.' },
	{ name: 'CLAUDE_CODE_DISABLE_WORKFLOWS', group: 'skills', tag: 'safe', notes: 'Disable dynamic + bundled workflows.' },
	{ name: 'CLAUDE_CODE_DISABLE_POLICY_SKILLS', group: 'skills', tag: 'care', notes: 'Skip the system-wide managed skills dir (=1).' },
	{ name: 'API_TIMEOUT_MS', group: 'timeouts', tag: 'safe', notes: 'API request timeout, default 600000 (10m).' },
	{ name: 'BASH_DEFAULT_TIMEOUT_MS', group: 'timeouts', tag: 'safe', notes: 'Default long bash timeout, default 120000 (2m).' },
	{ name: 'BASH_MAX_TIMEOUT_MS', group: 'timeouts', tag: 'safe', notes: 'Max bash timeout the model can set, default 600000.' },
	{ name: 'MCP_TIMEOUT', group: 'timeouts', tag: 'safe', notes: 'MCP server startup, default 30000.' },
	{ name: 'MCP_TOOL_TIMEOUT', group: 'timeouts', tag: 'safe', notes: 'MCP tool execution timeout.' },
	{ name: 'CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC', group: 'telemetry', tag: 'safe', notes: 'Umbrella: autoupdater + feedback + error-reporting + telemetry off (=1).' },
	{ name: 'DISABLE_TELEMETRY', group: 'telemetry', tag: 'safe', notes: 'Opt out of telemetry + GrowthBook flags (=1).' },
	{ name: 'DO_NOT_TRACK', group: 'telemetry', tag: 'safe', notes: 'Cross-tool opt-out == DISABLE_TELEMETRY (=1).' },
	{ name: 'DISABLE_ERROR_REPORTING', group: 'telemetry', tag: 'safe', notes: 'Disable error reporting (=1).' },
	{ name: 'DISABLE_AUTOUPDATER', group: 'telemetry', tag: 'safe', notes: 'Disable background auto-updates (=1).' },
	{ name: 'CLAUDE_CODE_DISABLE_FEEDBACK_SURVEY', group: 'telemetry', tag: 'safe', notes: 'Quality-survey prompts off (=1).' },
];

/** The "Quiet defaults" preset (catalog.toml `[[presets]]` id `quiet-defaults`). */
export const QUIET_DEFAULTS = {
	id: 'quiet-defaults',
	name: 'Quiet defaults',
	description:
		'Silence bundled skills/workflows/artifact + remote-control + telemetry/updater noise. All reversible per account.',
	// settings.json keys applied by the preset. Note: some (strictKnownMarketplaces,
	// strictPluginOnlyCustomization, disableSideloadFlags) are MANAGED keys the
	// server preset applies on cctui's behalf; the per-account editor validates
	// pasted settings and would reject those, so the webui preset only fills the
	// SAFE/CARE keys the operator can actually set here.
	settings: {
		disableBundledSkills: true,
		disableWorkflows: true,
		disableArtifact: true,
		disableRemoteControl: true,
		remoteControlAtStartup: false
	} as Record<string, unknown>,
	env: {
		CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC: '1',
		CLAUDE_CODE_DISABLE_BUNDLED_SKILLS: '1',
		CLAUDE_CODE_DISABLE_WORKFLOWS: '1',
		DISABLE_TELEMETRY: '1',
		DO_NOT_TRACK: '1',
		DISABLE_ERROR_REPORTING: '1',
		DISABLE_AUTOUPDATER: '1',
		CLAUDE_CODE_DISABLE_FEEDBACK_SURVEY: '1'
	} as Record<string, string>
};

const SETTINGS_KEY_SET = new Set(CATALOG_KEYS.map((k) => k.name));
const ENV_NAME_SET = new Set(CATALOG_ENV.map((e) => e.name));

/** Boolean SAFE/CARE keys, in catalog order, grouped for the toggle list. */
export const BOOL_KEYS = CATALOG_KEYS.filter((k) => k.kind === 'bool');

/** Distinct settings groups in first-seen order (for grouped rendering). */
export const SETTINGS_GROUPS: string[] = [...new Set(BOOL_KEYS.map((k) => k.group))];

/** Distinct env groups in first-seen order. */
export const ENV_GROUPS: string[] = [...new Set(CATALOG_ENV.map((e) => e.group))];

/** True when a settings key is one the toggle list renders a control for. */
export const isKnownBoolKey = (name: string): boolean =>
	CATALOG_KEYS.some((k) => k.name === name && k.kind === 'bool');

/** True when a settings key name is a known SAFE/CARE catalog key. */
export const isKnownSettingsKey = (name: string): boolean => SETTINGS_KEY_SET.has(name);

/** True when an env var name is in the curated allowlist. */
export const isAllowedEnv = (name: string): boolean => ENV_NAME_SET.has(name);

/**
 * Client-side validation mirroring the server allowlist (CCT-538). Returns the
 * offending top-level keys — settings keys that aren't SAFE/CARE catalog keys.
 * The server is authoritative; this just gives fast inline feedback.
 */
export function invalidSettingsKeys(obj: Record<string, unknown>): string[] {
	return Object.keys(obj).filter((k) => !isKnownSettingsKey(k));
}

/** Env var names not in the curated allowlist. */
export function invalidEnvKeys(names: string[]): string[] {
	return names.filter((n) => n.trim() && !isAllowedEnv(n.trim()));
}
