DELETE FROM skill_registry r USING skill_registry newer
WHERE r.name = newer.name AND r.uploaded_at < newer.uploaded_at;
ALTER TABLE skill_registry DROP CONSTRAINT IF EXISTS skill_registry_pkey;
ALTER TABLE skill_registry ALTER COLUMN uploaded_by_user DROP NOT NULL;
ALTER TABLE skill_registry ADD PRIMARY KEY (name);

DROP TABLE IF EXISTS spawn_tree_grants;
