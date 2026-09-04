<script lang="ts">
	// User settings. Server-persisted via the `settings` singleton (GET/PUT
	// /api/v1/settings, localStorage-mirrored). Grouped into sections —
	// Session list · Display · Harness · Notifications · Keyboard — matching
	// the settings catalogue.
	import { settings } from '$lib/settings.svelte';
	import { LOCALE_LABELS, LOCALES, type Locale } from '$lib/locale.svelte';
	import { AUTO, theme, THEMES } from '$lib/theme.svelte';
	import { fontScale, SCALE_LEVELS } from '$lib/fontscale.svelte';
	import { notify } from '$lib/notify.svelte';
	import {
		Button,
		Card,
		Heading,
		Input,
		Select,
		Stack,
		Switch,
		Text,
		Textarea,
		Field
	} from '@dorsk/tsumikit';
	import { useMe, useVersion, endpoints, qk } from '$lib/queries';
	import { useQueryClient } from '@tanstack/svelte-query';
	import { toasts } from '$lib/toast.svelte';
	import { m } from '$lib/paraglide/messages';
	import type { HarnessMode, WhipMode } from '$lib/settings.svelte';

	const sl = $derived(settings.state.sessionList);

	// Display section: the blob-backed wrappers on `settings` drive the runtime
	// theme/fontScale/notify singletons AND persist, so this panel and the header
	// share one round-tripping surface.
	function setTheme(id: string) {
		settings.setTheme(id);
	}
	function setFontScale(levelId: string) {
		settings.setFontScaleLevel(levelId);
	}
	async function toggleNotify() {
		if (notify.enabled) notify.disable();
		else await notify.enable();
		settings.recordNotifyEnabled();
	}
	function toggleNotifySound() {
		settings.setNotifySound(!notify.sound);
	}

	// Multi-value (csv) helpers for the label-id sets.
	function csv(ids: string[]): string {
		return ids.join(', ');
	}

	// Claude harness mode. Per-user; applies to all the user's machines and a
	// connected daemon switches within ~1s. Codex sessions ignore it.
	const harnessMode = $derived(settings.harnessMode);
	const harnessOpts: { v: HarnessMode; label: string; help: string }[] = [
		{
			v: 'bg',
			label: m.settings_harness_bg_label(),
			help: m.settings_harness_bg_help()
		},
		{
			v: 'sdk',
			label: m.settings_harness_sdk_label(),
			help: m.settings_harness_sdk_help()
		},
		{
			v: 'oneshot',
			label: m.settings_harness_oneshot_label(),
			help: m.settings_harness_oneshot_help()
		}
	];
	const harnessHelp = $derived(harnessOpts.find((o) => o.v === harnessMode)?.help ?? '');

	// Whip-mode stall-phrase override. `extend` appends to the daemon's
	// compiled defaults; `replace` swaps them. The phrase textarea is one phrase
	// per line; the server trims/lowercases/dedupes/caps on save.
	const whip = $derived(settings.whipStopPhrases);
	const whipPhrasesText = $derived(whip.phrases.join('\n'));
	function setWhipPhrasesText(text: string) {
		const phrases = text
			.split('\n')
			.map((p) => p.trim())
			.filter((p) => p.length > 0);
		settings.setWhipStopPhrases({ phrases });
	}
	// Mirrors the daemon's compiled STALL_PHRASES (crates/cctui-daemon/src/whipstop.rs)
	// so users can see what `extend` extends — kept in sync by hand (read-only view).
	const BUILTIN_STALL_PHRASES = [
		'out of scope',
		'not in scope',
		'beyond the scope',
		'left this for',
		'for a follow-up',
		'next session',
		'future session',
		'can be done later',
		'punting on',
		'pre-existing issue',
		'stopping here',
		'pausing here',
		'good stopping point',
		'natural stopping point',
		'good place to stop',
		'good checkpoint',
		'handing this back',
		'handing it back',
		'over to you',
		'your call',
		'let me know if',
		'let me know how',
		'feel free to',
		'ready for your review',
		'ready for review',
		'for your review',
		'waiting for your',
		'would you like me to',
		'do you want me to',
		'want me to',
		'shall i',
		'should i proceed',
		"if you'd like",
		'happy to continue',
		'happy to keep going'
	];

	// Emoji prefix on agent-generated session names. cctui does not generate the
	// names itself, so the prefix is added server-side when the name lands; a
	// name the user typed is left alone.
	const sessionEmojiPrefix = $derived(settings.sessionEmojiPrefix);

	// Server-wide instance name (admin only). Lives in `instance_settings` on
	// the server, not in the per-user blob; read back through /version so the
	// header and tab title pick it up on the next refetch.
	const me = useMe();
	const version = useVersion();
	const qc = useQueryClient();
	const isAdmin = $derived(me.data?.role === 'admin');
	let instanceDraft = $state('');
	let instanceSaving = $state(false);
	$effect(() => {
		instanceDraft = version.data?.instance_name ?? '';
	});
	const instanceDirty = $derived(instanceDraft.trim() !== (version.data?.instance_name ?? ''));

	// The server probes GitHub for a newer release every 6h; this asks it to go
	// now and swaps the cached `/version` payload with the answer, so the
	// header's update arrow reflects the result immediately.
	let updateChecking = $state(false);
	async function checkForUpdate() {
		updateChecking = true;
		try {
			const info = await endpoints.refreshVersion();
			qc.setQueryData(qk.version, info);
			toasts.ok(
				info.latest_version
					? m.settings_version_available({ version: info.latest_version })
					: m.settings_version_up_to_date()
			);
		} catch (e) {
			toasts.err(m.settings_version_check_failed({ error: e instanceof Error ? e.message : String(e) }));
		} finally {
			updateChecking = false;
		}
	}
	async function saveInstanceName() {
		instanceSaving = true;
		try {
			const res = await endpoints.updateInstance(instanceDraft.trim() || null);
			instanceDraft = res.name ?? '';
			await qc.invalidateQueries({ queryKey: qk.version });
			toasts.ok(res.name ? m.settings_admin_instance_saved() : m.settings_admin_instance_cleared());
		} catch (e) {
			toasts.err(e instanceof Error ? e.message : String(e));
		} finally {
			instanceSaving = false;
		}
	}
	const spawnDock = $derived(settings.spawnDock);
	const statsDock = $derived(settings.statsDock);

	// Daemon-side secret redaction. The switch toggles live scrubbing;
	// the textarea holds one extra regex per line, layered on the daemon's
	// compiled defaults. The server validates each regex on save.
	const scrubEnabled = $derived(settings.secretScrubEnabled);
	const scrubPatternsText = $derived(settings.secretScrubPatterns.map((p) => p.regex).join('\n'));
	function setScrubPatternsText(text: string) {
		const patterns = text
			.split('\n')
			.map((r) => r.trim())
			.filter((r) => r.length > 0)
			.map((regex) => ({ name: 'custom', regex, enabled: true }));
		settings.setSecretScrubPatterns(patterns);
	}
	const BUILTIN_SCRUB_CATEGORIES = [
		'github_token',
		'github_pat',
		'npm_token',
		'anthropic_key',
		'aws_access_key',
		'vault_token',
		'gitlab_token',
		'slack_token',
		'youtrack_token',
		'bitwarden_token',
		'cctui_token',
		'ccipat',
		'private_key',
		'jwt',
		'db_url_password'
	];
</script>

<Stack gap="lg">
	<header class="head">
		<Heading level={1}>{m.settings_title()}</Heading>
		<Text tone="faint">{m.settings_subtitle()}</Text>
	</header>

	<!-- ── Session list ─────────────────────────────────────────────────── -->
	<Card>
		<Stack gap="md">
			<Heading level={2}>{m.settings_session_list_title()}</Heading>
			<dl class="props">
				<div class="prop">
					<dt><Text weight="semibold">{m.settings_sort_label()}</Text></dt>
					<dd>
						<Select
							value={sl.sort}
							onchange={(e) =>
								settings.setSessionList({
									sort: (e.currentTarget as HTMLSelectElement).value as typeof sl.sort
								})}
						>
							<option value="activity">{m.settings_sort_activity()}</option>
							<option value="created">{m.settings_sort_created()}</option>
							<option value="name">{m.settings_sort_name()}</option>
						</Select>
					</dd>
				</div>
				<div class="prop">
					<dt><Text weight="semibold">{m.settings_view_label()}</Text></dt>
					<dd>
						<Select
							value={sl.view}
							onchange={(e) =>
								settings.setSessionList({
									view: (e.currentTarget as HTMLSelectElement).value as typeof sl.view
								})}
						>
							<option value="list">{m.settings_view_list()}</option>
							<option value="card">{m.settings_view_cards()}</option>
						</Select>
					</dd>
				</div>
				<div class="prop">
					<dt><Text weight="semibold">{m.settings_density_label()}</Text></dt>
					<dd>
						<Select
							value={sl.density}
							onchange={(e) =>
								settings.setSessionList({
									density: (e.currentTarget as HTMLSelectElement).value as typeof sl.density
								})}
						>
							<option value="normal">{m.settings_density_detailed()}</option>
							<option value="compact">{m.settings_density_compact()}</option>
						</Select>
					</dd>
				</div>
				<div class="prop">
					<dt>
						<Text weight="semibold">{m.settings_list_width_label()}</Text>
						<Text size="sm" tone="faint">{m.settings_list_width_help()}</Text>
					</dt>
					<dd>
						<Select
							value={sl.width}
							onchange={(e) =>
								settings.setSessionList({
									width: (e.currentTarget as HTMLSelectElement).value as typeof sl.width
								})}
						>
							<option value="default">{m.settings_list_width_default()}</option>
							<option value="wide">{m.settings_list_width_wide()}</option>
							<option value="ultra">{m.settings_list_width_ultra()}</option>
							<option value="full">{m.settings_list_width_full()}</option>
						</Select>
					</dd>
				</div>
				<div class="prop">
					<dt>
						<Text weight="semibold">{m.settings_account_names_label()}</Text>
						<Text size="sm" tone="faint">{m.settings_account_names_help()}</Text>
					</dt>
					<dd>
						<Switch
							checked={sl.accountNames}
							label={m.settings_account_names_label()}
							onclick={() => settings.setSessionList({ accountNames: !sl.accountNames })}
						/>
					</dd>
				</div>
				<div class="prop">
					<dt>
						<Text weight="semibold">{m.settings_sections_label()}</Text>
						<Text size="sm" tone="faint">{sl.section || m.common_all()}</Text>
					</dt>
					<dd></dd>
				</div>
				<div class="prop">
					<dt>
						<Text weight="semibold">{m.settings_label_filter_label()}</Text>
						<Text size="sm" tone="faint">{csv(sl.labelFilter) || m.common_none()}</Text>
					</dt>
					<dd></dd>
				</div>
				<div class="prop">
					<dt>
						<Text weight="semibold">{m.settings_session_emoji_label()}</Text>
						<Text size="sm" tone="faint">{m.settings_session_emoji_help()}</Text>
					</dt>
					<dd>
						<Switch
							checked={sessionEmojiPrefix}
							label={m.settings_session_emoji_label()}
							onclick={() => settings.setSessionEmojiPrefix(!sessionEmojiPrefix)}
						/>
					</dd>
				</div>
			</dl>
		</Stack>
	</Card>

	<!-- ── Instance (admin, server-wide) ────────────────────────────────── -->
	{#if isAdmin}
		<Card>
			<Stack gap="md">
				<Heading level={2}>{m.settings_admin_title()}</Heading>
				<dl class="props">
					<div class="prop">
						<dt>
							<Text weight="semibold">{m.settings_admin_instance_name_label()}</Text>
							<Text size="sm" tone="faint">{m.settings_admin_instance_name_help()}</Text>
						</dt>
						<dd class="inst-dd">
							<Input
								bind:value={instanceDraft}
								maxlength={48}
								placeholder={m.settings_admin_instance_name_placeholder()}
								onkeydown={(e: KeyboardEvent) => {
									if (e.key === 'Enter' && instanceDirty && !instanceSaving) saveInstanceName();
								}}
							/>
							<Button
								size="sm"
								disabled={!instanceDirty || instanceSaving}
								onclick={saveInstanceName}
							>
								{m.settings_admin_instance_save()}
							</Button>
						</dd>
					</div>
				</dl>
			</Stack>
		</Card>
	{/if}

	<!-- ── New session ──────────────────────────────────────────────────── -->
	<Card>
		<Stack gap="md">
			<Heading level={2}>{m.settings_spawn_title()}</Heading>
			<dl class="props">
				<div class="prop">
					<dt>
						<Text weight="semibold">{m.settings_spawn_dock_label()}</Text>
						<Text size="sm" tone="faint">{m.settings_spawn_dock_help()}</Text>
					</dt>
					<dd>
						<Switch
							checked={spawnDock.enabled}
							label={m.settings_spawn_dock_label()}
							onclick={() => settings.setSpawnDock({ enabled: !spawnDock.enabled })}
						/>
					</dd>
				</div>
				<div class="prop">
					<dt>
						<Text weight="semibold">{m.settings_spawn_dock_side_label()}</Text>
						<Text size="sm" tone="faint">{m.settings_spawn_dock_side_help()}</Text>
					</dt>
					<dd>
						<Select
							value={spawnDock.side}
							disabled={!spawnDock.enabled}
							onchange={(e) =>
								settings.setSpawnDock({
									side: (e.currentTarget as HTMLSelectElement).value as typeof spawnDock.side
								})}
						>
							<option value="left">{m.settings_spawn_dock_side_left()}</option>
							<option value="right">{m.settings_spawn_dock_side_right()}</option>
						</Select>
					</dd>
				</div>
			</dl>
		</Stack>
	</Card>

	<!-- ── Stats panel ──────────────────────────────────────────────────── -->
	<Card>
		<Stack gap="md">
			<Heading level={2}>{m.settings_stats_title()}</Heading>
			<dl class="props">
				<div class="prop">
					<dt>
						<Text weight="semibold">{m.settings_stats_dock_label()}</Text>
						<Text size="sm" tone="faint">{m.settings_stats_dock_help()}</Text>
					</dt>
					<dd>
						<Switch
							checked={statsDock.enabled}
							label={m.settings_stats_dock_label()}
							onclick={() => settings.setStatsDock({ enabled: !statsDock.enabled })}
						/>
					</dd>
				</div>
				<div class="prop">
					<dt>
						<Text weight="semibold">{m.settings_stats_dock_side_label()}</Text>
						<Text size="sm" tone="faint">{m.settings_stats_dock_side_help()}</Text>
					</dt>
					<dd>
						<Select
							value={statsDock.side}
							disabled={!statsDock.enabled}
							onchange={(e) =>
								settings.setStatsDock({
									side: (e.currentTarget as HTMLSelectElement).value as typeof statsDock.side
								})}
						>
							<option value="left">{m.settings_spawn_dock_side_left()}</option>
							<option value="right">{m.settings_spawn_dock_side_right()}</option>
						</Select>
					</dd>
				</div>
			</dl>
		</Stack>
	</Card>

	<!-- ── Display ──────────────────────────────────────────────────────── -->
	<Card>
		<Stack gap="md">
			<Heading level={2}>{m.settings_display_title()}</Heading>
			<dl class="props">
				<div class="prop">
					<dt><Text weight="semibold">{m.settings_theme_label()}</Text></dt>
					<dd>
						<Select
							value={theme.current}
							onchange={(e) => setTheme((e.currentTarget as HTMLSelectElement).value)}
						>
							<option value={AUTO.id}>{AUTO.icon} {m.nav_theme_auto()}</option>
							{#each THEMES as t (t.id)}
								<option value={t.id}>{t.icon} {t.label}</option>
							{/each}
						</Select>
					</dd>
				</div>
				<div class="prop">
					<dt><Text weight="semibold">{m.settings_font_size_label()}</Text></dt>
					<dd>
						<Select
							value={fontScale.levelId}
							onchange={(e) => setFontScale((e.currentTarget as HTMLSelectElement).value)}
						>
							{#each SCALE_LEVELS as l (l.id)}
								<option value={l.id}>{l.label}</option>
							{/each}
						</Select>
					</dd>
				</div>
				<div class="prop">
					<dt>
						<Text weight="semibold">{m.settings_archive_shortcut_label()}</Text>
						<Text size="sm" tone="faint">
							{m.settings_archive_shortcut_help()}
						</Text>
					</dt>
					<dd>
						<Switch
							checked={settings.state.display.archiveShortcut}
							label={m.settings_archive_shortcut_label()}
							onclick={() => settings.toggleArchiveShortcut()}
						/>
					</dd>
				</div>
			</dl>
		</Stack>
	</Card>

	<!-- ── Language ────────────────────────────────────────────── -->
	<Card>
		<Stack gap="md">
			<Heading level={2}>{m.settings_language_title()}</Heading>
			<dl class="props">
				<div class="prop">
					<dt>
						<Text weight="semibold">{m.settings_interface_language_label()}</Text>
						<Text size="sm" tone="faint">
							{m.settings_interface_language_help()}
						</Text>
					</dt>
					<dd>
						<Select
							value={settings.locale ?? 'auto'}
							onchange={(e) => {
								const v = (e.currentTarget as HTMLSelectElement).value;
								settings.setLocale(v === 'auto' ? null : (v as Locale));
							}}
						>
							<option value="auto">{m.settings_language_auto()}</option>
							{#each LOCALES as l (l)}
								<option value={l}>{LOCALE_LABELS[l]}</option>
							{/each}
						</Select>
					</dd>
				</div>
			</dl>
		</Stack>
	</Card>

	<!-- ── Claude harness mode ─────────────────────────────────────────── -->
	<Card>
		<Stack gap="md">
			<Heading level={2}>{m.settings_harness_title()}</Heading>
			<dl class="props">
				<div class="prop">
					<dt>
						<Text weight="semibold">{m.settings_harness_execution_label()}</Text>
						<Text size="sm" tone="faint">{harnessHelp}</Text>
						<Text size="sm" tone="faint">
							{m.settings_harness_execution_help()}
						</Text>
					</dt>
					<dd>
						<Select
							value={harnessMode}
							onchange={(e) =>
								settings.setHarnessMode((e.currentTarget as HTMLSelectElement).value as HarnessMode)}
						>
							{#each harnessOpts as o (o.v)}
								<option value={o.v}>{o.label}</option>
							{/each}
						</Select>
					</dd>
				</div>
			</dl>
		</Stack>
	</Card>

	<!-- ── Whip mode stall phrases ────────────────────────────── -->
	<Card>
		<Stack gap="md">
			<Heading level={2}>{m.settings_whip_title()}</Heading>
			<Text size="sm" tone="faint">
				{m.settings_whip_intro()}
			</Text>
			<dl class="props">
				<div class="prop">
					<dt>
						<Text weight="semibold">{m.settings_whip_phrase_list_label()}</Text>
						<Text size="sm" tone="faint">
							{whip.mode === 'replace'
								? m.settings_whip_mode_replace_help()
								: m.settings_whip_mode_extend_help()}
						</Text>
					</dt>
					<dd>
						<Select
							value={whip.mode}
							onchange={(e) =>
								settings.setWhipStopPhrases({
									mode: (e.currentTarget as HTMLSelectElement).value as WhipMode
								})}
						>
							<option value="extend">{m.settings_whip_mode_extend()}</option>
							<option value="replace">{m.settings_whip_mode_replace()}</option>
						</Select>
					</dd>
				</div>
			</dl>
			<Field label={m.settings_whip_phrases_field_label()}>
				<Textarea
					mono
					autoresize
					rows={4}
					value={whipPhrasesText}
					placeholder={'pour une autre session\nprêt pour ta relecture'}
					onchange={(e) => setWhipPhrasesText((e.currentTarget as HTMLTextAreaElement).value)}
				/>
			</Field>
			<Field
				label={m.settings_whip_guidance_label()}
				hint={m.settings_whip_guidance_hint()}
			>
				<Textarea
					autoresize
					rows={2}
					value={whip.guidance}
					onchange={(e) =>
						settings.setWhipStopPhrases({
							guidance: (e.currentTarget as HTMLTextAreaElement).value.trim()
						})}
				/>
			</Field>
			<details class="defaults">
				<summary><Text size="sm" tone="faint">{m.settings_whip_defaults_summary()}</Text></summary>
				<ul>
					{#each BUILTIN_STALL_PHRASES as p (p)}
						<li><Text size="sm" tone="faint">{p}</Text></li>
					{/each}
				</ul>
			</details>
		</Stack>
	</Card>

	<!-- ── Secret redaction ───────────────────────────────────── -->
	<Card>
		<Stack gap="md">
			<Heading level={2}>{m.settings_redaction_title()}</Heading>
			<Text size="sm" tone="faint">{m.settings_redaction_help()}</Text>
			<dl class="props">
				<div class="prop">
					<dt>
						<Text weight="semibold">{m.settings_redaction_enable_label()}</Text>
						<Text size="sm" tone="faint">{m.settings_redaction_enable_help()}</Text>
					</dt>
					<dd>
						<Switch
							checked={scrubEnabled}
							label={m.settings_redaction_enable_label()}
							onclick={() => settings.setSecretScrubEnabled(!scrubEnabled)}
						/>
					</dd>
				</div>
			</dl>
			<Field
				label={m.settings_redaction_patterns_label()}
				hint={m.settings_redaction_patterns_hint()}
			>
				<Textarea
					mono
					autoresize
					rows={3}
					value={scrubPatternsText}
					placeholder={'ACME-[0-9]{6}\\nMYCORP_[A-Za-z0-9]{20,}'}
					onchange={(e) => setScrubPatternsText((e.currentTarget as HTMLTextAreaElement).value)}
				/>
			</Field>
			<details class="defaults">
				<summary><Text size="sm" tone="faint">{m.settings_redaction_builtins_summary()}</Text></summary>
				<ul>
					{#each BUILTIN_SCRUB_CATEGORIES as c (c)}
						<li><Text size="sm" tone="faint">{c}</Text></li>
					{/each}
				</ul>
			</details>
		</Stack>
	</Card>

	<!-- ── Notifications ────────────────────────────────────────────────── -->
	<Card>
		<Stack gap="md">
			<Heading level={2}>{m.settings_notifications_title()}</Heading>
			<dl class="props">
				<div class="prop">
					<dt>
						<Text weight="semibold">{m.settings_notify_input_label()}</Text>
						<Text size="sm" tone="faint">
							{m.settings_notify_input_help()}
						</Text>
					</dt>
					<dd>
						<Switch
							checked={notify.enabled}
							label={m.settings_notifications_title()}
							disabled={!notify.supported}
							onclick={() => void toggleNotify()}
						/>
					</dd>
				</div>
				<div class="prop">
					<dt><Text weight="semibold">{m.settings_sound_label()}</Text></dt>
					<dd>
						<Switch checked={notify.sound} label={m.settings_notification_sound_label()} onclick={toggleNotifySound} />
					</dd>
				</div>
			</dl>
		</Stack>
	</Card>

	<!-- ── Keyboard ─────────────────────────────────────────────────────── -->
	<Card>
		<Stack gap="md">
			<Heading level={2}>{m.settings_keyboard_title()}</Heading>
			<Text tone="faint">{m.settings_keyboard_soon()}</Text>
		</Stack>
	</Card>

	<!-- ── Version ──────────────────────────────────────────────────────── -->
	<Card>
		<Stack gap="md">
			<Heading level={2}>{m.settings_version_title()}</Heading>
			<dl class="props">
				<div class="prop">
					<dt>
						<Text weight="semibold">{m.settings_version_server_label()}</Text>
						<Text size="sm" tone="faint">{m.settings_version_check_help()}</Text>
					</dt>
					<dd class="inst-dd">
						{#if version.data}
							<Text size="sm" variant="code">v{version.data.version}</Text>
							{#if version.data.latest_version}
								<a
									class="ver-link"
									href={version.data.latest_url ?? version.data.repo_url}
									target="_blank"
									rel="noopener"
								>
									<Text size="sm" variant="code">
										↑ v{version.data.latest_version}
									</Text>
								</a>
							{/if}
						{/if}
						<Button size="sm" disabled={updateChecking} onclick={checkForUpdate}>
							{updateChecking ? m.settings_version_checking() : m.settings_version_check()}
						</Button>
					</dd>
				</div>
			</dl>
		</Stack>
	</Card>
</Stack>

<style>
	.head {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
	}
	/* Each card: the section heading gets its own breathing room above the
	   rows, and each row is a label/help pair centered against its control,
	   separated by a hairline with even padding on both sides. */
	.props {
		display: flex;
		flex-direction: column;
		gap: 0;
		margin: var(--sp-3) 0 0;
	}
	.prop {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--sp-4);
		padding: var(--sp-3) 0;
	}
	.prop:first-child {
		padding-top: 0;
	}
	.prop:last-child {
		padding-bottom: 0;
	}
	.prop dt {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
		min-width: 0;
		max-width: 40rem;
	}
	.prop dd {
		margin: 0;
		flex: none;
	}
	.inst-dd {
		display: flex;
		gap: var(--sp-2);
		align-items: center;
	}
	/* Same red-arrow cue as the header badge, so "a newer release exists" reads
	   the same in both places. */
	.ver-link {
		color: var(--danger);
		text-decoration: none;
	}
	.ver-link:hover {
		text-decoration: underline;
	}
	.prop + .prop {
		border-top: 1px solid var(--border);
	}
	.defaults ul {
		display: flex;
		flex-wrap: wrap;
		gap: var(--sp-1) var(--sp-3);
		margin: var(--sp-2) 0 0;
		padding: 0 0 0 var(--sp-3);
	}
	.defaults summary {
		cursor: pointer;
	}
</style>
