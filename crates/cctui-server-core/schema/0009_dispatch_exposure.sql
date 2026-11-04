-- Rule of Two exposure the run's worker profile declared, as its dispatcher
-- reported it: {"untrusted_input", "sensitive_data", "outward", "human_gate"?}.
ALTER TABLE public.dispatch_records ADD COLUMN IF NOT EXISTS exposure jsonb;
