-- Optional protocol features a dispatcher announced on its current connection,
-- readable from every replica.
ALTER TABLE public.dispatchers ADD COLUMN IF NOT EXISTS features text[] NOT NULL DEFAULT '{}';
