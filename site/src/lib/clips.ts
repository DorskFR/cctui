export const CLIP_SLUGS = [
  'hero',
  'live-session',
  'permissions',
  'spawn',
  'fleet',
  'harnesses',
  'subagents',
  'rooms',
  'voice',
  'workers',
  'usage',
  'search',
  'plugins',
  'themes',
  'mobile',
  'tui',
] as const;

export type ClipSlug = (typeof CLIP_SLUGS)[number];
