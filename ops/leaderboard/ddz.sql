-- Dou Dizhu daily leaderboard. Safe to run again on an existing database.
BEGIN;

CREATE TABLE IF NOT EXISTS wzq_private.ddz_matches (
    id text PRIMARY KEY,
    day date NOT NULL
);

CREATE TABLE IF NOT EXISTS wzq_private.ddz_daily_scores (
    day date NOT NULL,
    user_id bigint NOT NULL,
    name text NOT NULL,
    games integer NOT NULL,
    wins integer NOT NULL,
    points bigint NOT NULL,
    PRIMARY KEY (day, user_id)
);

-- p_players holds three entries, one per seat: null for a guest, otherwise
-- {"id", "name", "delta", "won"}.
CREATE OR REPLACE FUNCTION wzq_api.record_ddz_match(
    p_id text, p_finished_at bigint, p_players jsonb
) RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
DECLARE
    match_day date := (to_timestamp(p_finished_at) AT TIME ZONE 'Asia/Shanghai')::date;
BEGIN
    IF p_id !~ '^[a-f0-9]{32}$' OR jsonb_typeof(p_players) <> 'array'
        OR jsonb_array_length(p_players) <> 3
        OR EXISTS (
            SELECT 1 FROM jsonb_array_elements(p_players) AS players(player)
            WHERE player <> 'null'::jsonb
                AND (abs((player->>'delta')::bigint) > 128 OR (player->>'id')::bigint IS NULL)
        ) THEN
        RAISE EXCEPTION 'Invalid ranked dou dizhu match';
    END IF;

    INSERT INTO wzq_private.ddz_matches (id, day) VALUES (p_id, match_day)
    ON CONFLICT (id) DO NOTHING;
    IF NOT FOUND THEN
        RETURN;
    END IF;

    INSERT INTO wzq_private.ddz_daily_scores AS scores (day, user_id, name, games, wins, points)
    SELECT match_day, (player->>'id')::bigint, max(player->>'name'), count(*)::integer,
        count(*) FILTER (WHERE (player->>'won')::boolean)::integer,
        sum((player->>'delta')::bigint)
    FROM jsonb_array_elements(p_players) AS players(player)
    WHERE player <> 'null'::jsonb
    GROUP BY (player->>'id')::bigint
    ON CONFLICT (day, user_id) DO UPDATE SET
        name = EXCLUDED.name,
        games = scores.games + EXCLUDED.games,
        wins = scores.wins + EXCLUDED.wins,
        points = scores.points + EXCLUDED.points;
END;
$$;

CREATE OR REPLACE FUNCTION wzq_api.ddz_daily_leaderboard(p_viewer bigint) RETURNS jsonb
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    WITH today AS (
        SELECT (statement_timestamp() AT TIME ZONE 'Asia/Shanghai')::date AS day
    ), ranked AS (
        SELECT user_id, name, games, wins, games - wins AS losses, points,
            row_number() OVER (ORDER BY points DESC, wins DESC, user_id) AS rank
        FROM wzq_private.ddz_daily_scores
        WHERE day = (SELECT day FROM today)
    ), public_rows AS (
        SELECT user_id, rank, to_jsonb(ranked) - 'user_id' AS payload
        FROM ranked
    )
    SELECT jsonb_build_object(
        'day', (SELECT day FROM today),
        'entries', coalesce((SELECT jsonb_agg(payload ORDER BY rank) FROM public_rows WHERE rank <= 20), '[]'::jsonb),
        'you', (SELECT payload FROM public_rows WHERE user_id = p_viewer)
    );
$$;

CREATE OR REPLACE FUNCTION wzq_api.ddz_previous_day_champion() RETURNS jsonb
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
            SELECT jsonb_build_object('name', name, 'points', points)
            FROM wzq_private.ddz_daily_scores
            WHERE day = today - 1
            ORDER BY points DESC, wins DESC, user_id
            LIMIT 1
        )
    )
    FROM calendar;
$$;

REVOKE ALL ON FUNCTION wzq_api.record_ddz_match(text, bigint, jsonb) FROM PUBLIC;
REVOKE ALL ON FUNCTION wzq_api.ddz_daily_leaderboard(bigint) FROM PUBLIC;
REVOKE ALL ON FUNCTION wzq_api.ddz_previous_day_champion() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION wzq_api.record_ddz_match(text, bigint, jsonb) TO wzq_app;
GRANT EXECUTE ON FUNCTION wzq_api.ddz_daily_leaderboard(bigint) TO wzq_app;
GRANT EXECUTE ON FUNCTION wzq_api.ddz_previous_day_champion() TO wzq_app;
NOTIFY pgrst, 'reload schema';

COMMIT;
