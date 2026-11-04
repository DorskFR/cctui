-- When the dispatcher first answered that the handle's workload no longer
-- exists. Liveness probes skip such a handle until a redispatch replaces it.
ALTER TABLE public.dispatch_handles ADD COLUMN IF NOT EXISTS gone_at timestamp with time zone;
