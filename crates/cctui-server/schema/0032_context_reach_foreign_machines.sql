-- A non-admin item can never reach another user's machine, so those ids are
-- dropped. An item left with no machine is disabled: an empty list would
-- widen it to every machine of its owner. Admin items keep theirs until the
-- admin sets reach or drops them.
DO $$
DECLARE
    r record;
    n_paths int;
    n_labels int;
    n_adapters int;
    next_scope text;
    next_ref text;
BEGIN
    FOR r IN
        SELECT c.id, c.user_id, c.filters, c.enabled,
               array(SELECT v FROM jsonb_array_elements_text(c.filters->'machines') v
                      WHERE NOT EXISTS (SELECT 1 FROM public.machines m
                                         WHERE m.id::text = lower(v) AND m.user_id <> c.user_id)
                      ORDER BY v) AS kept,
               array(SELECT v FROM jsonb_array_elements_text(c.filters->'machines') v
                      WHERE EXISTS (SELECT 1 FROM public.machines m
                                     WHERE m.id::text = lower(v) AND m.user_id <> c.user_id)
                      ORDER BY v) AS dropped
          FROM public.context_items c
         WHERE jsonb_typeof(c.filters->'machines') = 'array'
           AND jsonb_array_length(c.filters->'machines') > 0
           AND NOT EXISTS (SELECT 1 FROM public.user_acls a
                            WHERE a.user_id = c.user_id AND a.scope = 'admin')
         ORDER BY c.id
    LOOP
        CONTINUE WHEN cardinality(r.dropped) = 0;
        n_paths := coalesce(jsonb_array_length(r.filters->'paths'), 0);
        n_labels := coalesce(jsonb_array_length(r.filters->'labels'), 0);
        n_adapters := coalesce(jsonb_array_length(r.filters->'adapters'), 0);
        next_scope := CASE
            WHEN n_adapters > 0 THEN 'user'
            WHEN cardinality(r.kept) = 1 AND n_paths = 0 AND n_labels = 0 THEN 'machine'
            WHEN cardinality(r.kept) = 0 AND n_paths = 1 AND n_labels = 0 THEN 'path'
            WHEN cardinality(r.kept) = 0 AND n_paths = 0 AND n_labels = 1 THEN 'label'
            ELSE 'user'
        END;
        next_ref := CASE next_scope
            WHEN 'machine' THEN r.kept[1]
            WHEN 'path' THEN r.filters->'paths'->>0
            WHEN 'label' THEN r.filters->'labels'->>0
        END;
        RAISE WARNING 'context item % of user %: dropped other users'' machines %; %',
            r.id, r.user_id, r.dropped,
            CASE WHEN cardinality(r.kept) = 0 AND r.enabled THEN 'no machine left, disabled'
                 ELSE 'kept ' || cardinality(r.kept) || ' machine(s)' END;
        UPDATE public.context_items
           SET filters = jsonb_set(r.filters, '{machines}', to_jsonb(r.kept)),
               scope = next_scope,
               scope_ref = next_ref,
               enabled = enabled AND cardinality(r.kept) > 0,
               updated_at = now()
         WHERE id = r.id;
    END LOOP;
END $$;
