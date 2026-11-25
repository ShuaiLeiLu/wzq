BEGIN;

CREATE ROLE wzq_authenticator LOGIN NOINHERIT PASSWORD :'authenticator_password';
CREATE ROLE wzq_app NOLOGIN;
GRANT wzq_app TO wzq_authenticator;

CREATE SCHEMA wzq_private;
CREATE SCHEMA wzq_api;
REVOKE ALL ON SCHEMA wzq_private, wzq_api FROM PUBLIC;
GRANT USAGE ON SCHEMA wzq_api TO wzq_app;

CREATE TABLE wzq_private.ranked_matches (
    id text PRIMARY KEY,
    day date NOT NULL
);

CREATE TABLE wzq_private.daily_scores (
    day date NOT NULL,
    user_id bigint NOT NULL,
    name text NOT NULL,
    wins integer NOT NULL,
    losses integer NOT NULL,
    PRIMARY KEY (day, user_id)
);

CREATE FUNCTION wzq_api.record_ranked_match(
    p_id text, p_finished_at bigint, p_winner integer, p_players jsonb
) RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
DECLARE
    match_day date := (to_timestamp(p_finished_at) AT TIME ZONE 'Asia/Shanghai')::date;
BEGIN
    IF p_id !~ '^[a-f0-9]{32}$' OR jsonb_array_length(p_players) <> 2
        OR p_winner NOT IN (1, 2) THEN
        RAISE EXCEPTION 'Invalid ranked match';
    END IF;

    INSERT INTO wzq_private.ranked_matches (id, day) VALUES (p_id, match_day)
    ON CONFLICT (id) DO NOTHING;
    IF NOT FOUND THEN
        RETURN;
    END IF;

    INSERT INTO wzq_private.daily_scores AS scores (day, user_id, name, wins, losses)
    SELECT match_day, (player->>'id')::bigint, max(player->>'name'),
        count(*) FILTER (WHERE seat = p_winner)::integer,
        count(*) FILTER (WHERE seat <> p_winner)::integer
    FROM jsonb_array_elements(p_players) WITH ORDINALITY AS players(player, seat)
    WHERE player <> 'null'::jsonb
    GROUP BY (player->>'id')::bigint
    ON CONFLICT (day, user_id) DO UPDATE SET
        name = EXCLUDED.name,
        wins = scores.wins + EXCLUDED.wins,
        losses = scores.losses + EXCLUDED.losses;
END;
$$;

CREATE FUNCTION wzq_api.daily_leaderboard(p_viewer bigint) RETURNS jsonb
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    WITH today AS (
        SELECT (statement_timestamp() AT TIME ZONE 'Asia/Shanghai')::date AS day
    ), ranked AS (
        SELECT user_id, name, wins, losses, wins - losses AS points,
            row_number() OVER (ORDER BY wins - losses DESC, wins DESC, user_id) AS rank
        FROM wzq_private.daily_scores
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

REVOKE ALL ON ALL FUNCTIONS IN SCHEMA wzq_api FROM PUBLIC;
GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA wzq_api TO wzq_app;

COMMIT;
