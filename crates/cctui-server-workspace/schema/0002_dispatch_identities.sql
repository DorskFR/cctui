-- TABLE: dispatch_identities
CREATE TABLE public.dispatch_identities (
    dispatch_id uuid PRIMARY KEY,
    dispatcher_id uuid NOT NULL,
    user_id uuid NOT NULL,
    session_id text NOT NULL,
    namespace text NOT NULL,
    job_name text NOT NULL,
    machine_key_hash text NOT NULL,
    encrypted_machine_key text,
    encrypted_env text,
    issuer text,
    pod_name text,
    pod_uid text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    redeemed_at timestamp with time zone,
    env_read_at timestamp with time zone,
    expires_at timestamp with time zone NOT NULL
);

CREATE INDEX dispatch_identities_expires_at_idx ON public.dispatch_identities USING btree (expires_at);
