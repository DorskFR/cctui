-- Context the approval page needs to tell whose device is asking: a device
-- login is initiated by an unauthenticated caller, and client_name is whatever
-- that caller chose to send.
ALTER TABLE device_auth_requests ADD COLUMN IF NOT EXISTS client_ip  TEXT;
ALTER TABLE device_auth_requests ADD COLUMN IF NOT EXISTS user_agent TEXT;
