import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

export default defineConfig({
  site: 'https://dorskfr.github.io',
  base: '/cctui',
  trailingSlash: 'always',
  integrations: [
    starlight({
      title: 'cctui',
      description:
        'Self-hosted control plane for Claude Code, Codex, opencode and ACP agent sessions across all your machines.',
      logo: { src: './src/assets/logo.svg', alt: 'cctui' },
      favicon: '/favicon.svg',
      social: [{ icon: 'github', label: 'GitHub', href: 'https://github.com/DorskFR/cctui' }],
      customCss: ['./src/styles/starlight.css'],
      editLink: { baseUrl: 'https://github.com/DorskFR/cctui/edit/main/site/' },
      lastUpdated: false,
      sidebar: [
        {
          label: 'Getting started',
          items: [
            { label: 'Quick start', slug: 'getting-started/quick-start' },
            { label: 'Enroll a machine', slug: 'getting-started/enroll-a-machine' },
            { label: 'Run on Kubernetes workers', slug: 'getting-started/kubernetes-workers' },
            { label: 'Configuration', slug: 'getting-started/configuration' },
          ],
        },
        {
          label: 'Features',
          items: [
            { label: 'Every harness, one window', slug: 'features/harnesses' },
            { label: 'Live sessions', slug: 'features/live-sessions' },
            { label: 'Permissions and prompts', slug: 'features/permissions' },
            { label: 'Fleet, spawn and dispatch', slug: 'features/fleet' },
            { label: 'Ephemeral workers and guard proxy', slug: 'features/workers' },
            { label: 'Agents that spawn agents', slug: 'features/subagents' },
            { label: 'Peers and rooms', slug: 'features/rooms' },
            { label: 'cctuiverse', slug: 'features/cctuiverse' },
            { label: 'Voice', slug: 'features/voice' },
            { label: 'Mods, plugins and apps', slug: 'features/plugins' },
            { label: 'Yubisashi', slug: 'features/yubisashi' },
            { label: 'Accounts, usage and caps', slug: 'features/usage' },
            { label: 'Search, archive and context', slug: 'features/search' },
            { label: 'Security', slug: 'features/security' },
            { label: 'Mobile, themes and i18n', slug: 'features/themes-mobile' },
            { label: 'CLI, TUI and admin', slug: 'features/cli-tui' },
            { label: 'Self-update and channels', slug: 'features/self-update' },
          ],
        },
        { label: 'Architecture', slug: 'architecture' },
        { label: 'Comparison', slug: 'comparison' },
        {
          label: 'Reference',
          items: [
            { label: 'Command line', slug: 'reference/cli' },
            { label: 'MCP tools for agents', slug: 'reference/mcp-tools' },
          ],
        },
        { label: 'Changelog highlights', slug: 'changelog' },
      ],
    }),
  ],
});
