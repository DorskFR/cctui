-- Consecutive 404s from the peer: a link closes only after several, spread
-- out, because the receiver's uniform 404 also covers clock skew and redeploys.
ALTER TABLE cctuiverse_links ADD COLUMN IF NOT EXISTS peer_404_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE cctuiverse_links ADD COLUMN IF NOT EXISTS peer_404_since TIMESTAMPTZ;
