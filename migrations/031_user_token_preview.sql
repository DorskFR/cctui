-- CCT-185: store a non-secret preview of each user token so the admin UI can
-- show a recognisable fragment (e.g. `cctui_u_ab12…ef34`) without ever exposing
-- the full secret. NULL for tokens minted before this migration → UI masks them.
ALTER TABLE user_tokens ADD COLUMN IF NOT EXISTS token_preview TEXT;
