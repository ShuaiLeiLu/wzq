BEGIN;

CREATE OR REPLACE FUNCTION wzq_api.previous_day_champion() RETURNS jsonb
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    WITH calendar AS (
        SELECT (statement_timestamp() AT TIME ZONE 'Asia/Shanghai')::date AS today
    )
    SELECT jsonb_build_object(
        'day', today - 1,
        'validUntil', extract(epoch FROM ((today + 1)::timestamp AT TIME ZONE 'Asia/Shanghai'))::bigint,
        'winner', (
            SELECT jsonb_build_object('name', name, 'points', wins - losses)
            FROM wzq_private.daily_scores
            WHERE day = today - 1
            ORDER BY wins - losses DESC, wins DESC, user_id
            LIMIT 1
        )
    )
    FROM calendar;
$$;

REVOKE ALL ON FUNCTION wzq_api.previous_day_champion() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION wzq_api.previous_day_champion() TO wzq_app;
NOTIFY pgrst, 'reload schema';

COMMIT;
