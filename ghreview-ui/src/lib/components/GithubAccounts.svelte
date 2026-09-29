<script lang="ts">
  import {
    Button,
    Callout,
    Card,
    Cluster,
    EmptyState,
    Field,
    Heading,
    IconButton,
    Input,
    Stack,
    Text,
  } from "@dorsk/tsumikit";
  import { createQuery, useQueryClient } from "@tanstack/svelte-query";
  import { type AccountSummary, ApiError, api } from "../api/client";
  import { getAccount, setAccount } from "../api/config";
  import { keys } from "../api/queries";

  const client = useQueryClient();

  const query = createQuery(() => ({
    queryKey: keys.accounts(),
    queryFn: () => api.accounts(),
  }));

  const accounts = $derived<AccountSummary[]>(query.data?.items ?? []);

  let token = $state("");
  let login = $state("");
  let saving = $state(false);
  let addError = $state<string | null>(null);
  let removing = $state<string | null>(null);
  let removeError = $state<string | null>(null);

  function message(error: unknown): string {
    if (error instanceof ApiError) return error.message;
    return error instanceof Error ? error.message : String(error);
  }

  async function add(e: SubmitEvent): Promise<void> {
    e.preventDefault();
    if (saving || !token.trim()) return;
    saving = true;
    addError = null;
    try {
      const created = await api.addAccount({
        token: token.trim(),
        login: login.trim() || undefined,
      });
      token = "";
      login = "";
      if (!getAccount()) setAccount(created.login);
      await client.invalidateQueries({ queryKey: keys.accounts() });
    } catch (error) {
      addError = message(error);
    } finally {
      saving = false;
    }
  }

  async function remove(account: AccountSummary): Promise<void> {
    if (removing) return;
    removing = account.id;
    removeError = null;
    try {
      await api.removeAccount(account.id);
      if (getAccount() === account.login) setAccount(null);
      await client.invalidateQueries({ queryKey: keys.accounts() });
    } catch (error) {
      removeError = message(error);
    } finally {
      removing = null;
    }
  }
</script>

<div class="accounts">
  <Stack gap="var(--gh-space-4)">
    <Stack gap="var(--gh-space-2)">
      <Heading level={2} size="md">GitHub accounts</Heading>
      <Text size="sm" tone="muted">
        The review backend polls GitHub with these tokens. They are sealed on the backend and never
        returned.
      </Text>
    </Stack>

    {#if query.isLoading}
      <EmptyState size="inline" loading title="Loading accounts…" />
    {:else if query.isError}
      <Callout tone="danger">{message(query.error)}</Callout>
    {:else if accounts.length === 0}
      <EmptyState
        size="compact"
        icon="user"
        title="No GitHub account connected"
        description="Add a personal access token below to start syncing pull requests."
      />
    {:else}
      <ul class="list">
        {#each accounts as account (account.id)}
          <li>
            <Cluster gap="var(--gh-space-3)" align="center">
              <Text weight="medium">{account.login}</Text>
              {#if !account.active}<Text size="xs" tone="muted">inactive</Text>{/if}
              <div class="spacer"></div>
              <IconButton
                icon="trash"
                label="Remove {account.login}"
                size={16}
                hoverDanger
                disabled={removing !== null}
                onclick={() => remove(account)}
              />
            </Cluster>
          </li>
        {/each}
      </ul>
      {#if removeError}
        <Callout tone="danger">{removeError}</Callout>
      {/if}
    {/if}

    <Card padding="md">
      <form onsubmit={add}>
        <Stack gap="var(--gh-space-3)">
          <Heading level={3} size="sm">Add an account</Heading>
          <Field label="Personal access token" hint="Fine-grained tokens are preferred.">
            <Input
              type="password"
              bind:value={token}
              placeholder="github_pat_…"
              spellcheck="false"
              autocomplete="off"
              disabled={saving}
            />
          </Field>
          <Field label="Login" hint="Optional; rejected when it does not match the token.">
            <Input
              type="text"
              bind:value={login}
              placeholder="DorskFR"
              spellcheck="false"
              disabled={saving}
            />
          </Field>
          {#if addError}
            <Callout tone="danger">{addError}</Callout>
          {/if}
          <Button type="submit" variant="primary" loading={saving} disabled={!token.trim()}>
            Add account
          </Button>
        </Stack>
      </form>
    </Card>
  </Stack>
</div>

<style>
  .accounts {
    padding: var(--gh-space-4);
    max-width: 48rem;
  }
  .list {
    list-style: none;
    margin: 0;
    padding: 0;
    border: 1px solid var(--gh-border);
    border-radius: var(--gh-radius);
  }
  li {
    padding: var(--gh-space-2) var(--gh-space-3);
    border-bottom: 1px solid var(--gh-border-muted);
  }
  li:last-child {
    border-bottom: none;
  }
  .spacer {
    flex: 1;
  }
</style>
