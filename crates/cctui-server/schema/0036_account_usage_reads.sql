-- One provider usage read per account at a time across every replica:
-- `read_after` is when the next read may start, `failures` drives the shared
-- backoff, and `reading` is the last successful read, served by the others.
CREATE TABLE public.account_usage_reads (
    account_id uuid        PRIMARY KEY REFERENCES public.account_providers(id) ON DELETE CASCADE,
    read_after timestamptz NOT NULL,
    failures   integer     NOT NULL DEFAULT 0,
    reading    jsonb,
    read_at    timestamptz
);
