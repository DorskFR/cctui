ALTER TABLE presets
    ADD COLUMN account_id UUID REFERENCES accounts(id) ON DELETE SET NULL,
    ADD COLUMN pool_id    UUID REFERENCES account_pools(id) ON DELETE SET NULL,
    ADD COLUMN no_account BOOLEAN NOT NULL DEFAULT false,
    ADD CONSTRAINT presets_one_account_pick
        CHECK (num_nonnulls(account_id, pool_id) + no_account::int <= 1),
    ADD CONSTRAINT presets_account_pick_machine_only
        CHECK (machine_id IS NOT NULL
               OR (account_id IS NULL AND pool_id IS NULL AND NOT no_account));
