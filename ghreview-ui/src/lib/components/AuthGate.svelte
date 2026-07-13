<script lang="ts">
  import { getAccount, setAccount, setToken } from "../api/config";

  interface Props {
    onauthed: () => void;
  }
  let { onauthed }: Props = $props();

  let token = $state("");
  let account = $state(getAccount() ?? "");

  function save(e: SubmitEvent): void {
    e.preventDefault();
    if (!token.trim()) return;
    setToken(token.trim());
    setAccount(account.trim() || null);
    onauthed();
  }
</script>

<div class="gate">
  <form onsubmit={save}>
    <h1>gh-review</h1>
    <p>Enter a cctui bearer token to connect to the review backend.</p>
    <label>
      Bearer token
      <input type="password" bind:value={token} spellcheck="false" autocomplete="off" />
    </label>
    <label>
      Account (optional)
      <input type="text" bind:value={account} placeholder="DorskFR" spellcheck="false" />
    </label>
    <button type="submit">Connect</button>
    <small>Stored in localStorage. CCT-610 wires real cctui auth.</small>
  </form>
</div>

<style>
  .gate {
    flex: 1;
    display: grid;
    place-items: center;
  }
  form {
    display: flex;
    flex-direction: column;
    gap: var(--gh-space-3);
    width: 320px;
    background: var(--gh-bg-elev);
    border: 1px solid var(--gh-border);
    border-radius: var(--gh-radius);
    padding: var(--gh-space-4);
  }
  h1 {
    margin: 0;
    font-size: var(--fs-lg);
  }
  p {
    margin: 0;
    color: var(--gh-fg-muted);
  }
  label {
    display: flex;
    flex-direction: column;
    gap: var(--gh-space-1);
    font-size: var(--fs-xs);
    color: var(--gh-fg-muted);
  }
  input {
    background: var(--gh-bg-inset);
    border: 1px solid var(--gh-border);
    border-radius: var(--gh-radius);
    color: var(--gh-fg);
    padding: var(--gh-space-2);
  }
  button {
    background: var(--gh-accent);
    border: none;
    border-radius: var(--gh-radius);
    color: white;
    padding: var(--gh-space-2);
    cursor: pointer;
    font-weight: 600;
  }
  small {
    color: var(--gh-fg-subtle);
  }
</style>
