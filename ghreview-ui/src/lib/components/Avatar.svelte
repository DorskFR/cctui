<script lang="ts" module>
  import type { GithubUser } from "../api/types";

  export function initialOf(login: string | undefined | null): string {
    const c = (login ?? "").trim().charAt(0);
    return c ? c.toUpperCase() : "?";
  }

  export function sizedAvatarUrl(url: string | undefined | null, px: number): string | null {
    if (!url) return null;
    const scaled = Math.max(1, Math.round(px * 2));
    try {
      const u = new URL(url);
      u.searchParams.set("s", String(scaled));
      return u.toString();
    } catch {
      const sep = url.includes("?") ? "&" : "?";
      return `${url}${sep}s=${scaled}`;
    }
  }
</script>

<script lang="ts">
  interface Props {
    user?: GithubUser | null;
    size?: number;
  }
  const { user, size = 20 }: Props = $props();

  const src = $derived(sizedAvatarUrl(user?.avatar_url, size));
  const login = $derived(user?.login ?? "");
</script>

<span
  class="avatar"
  style="--sz: {size}px"
  title={login || undefined}
  aria-label={login || undefined}
>
  {#if src}
    <img {src} alt="" width={size} height={size} loading="lazy" decoding="async" />
  {:else}
    <span class="initial" aria-hidden="true">{initialOf(login)}</span>
  {/if}
</span>

<style>
  .avatar {
    flex: none;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: var(--sz);
    height: var(--sz);
    border-radius: 50%;
    overflow: hidden;
    background: var(--gh-bg-inset);
    border: 1px solid var(--gh-border-muted);
    color: var(--gh-fg-muted);
    vertical-align: middle;
  }
  img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }
  .initial {
    font-size: calc(var(--sz) * 0.55);
    line-height: 1;
    font-weight: 600;
    text-transform: uppercase;
  }
</style>
