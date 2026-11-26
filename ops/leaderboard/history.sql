BEGIN;
CREATE TABLE IF NOT EXISTS wzq_private.game_history (
    id text PRIMARY KEY,
    game text NOT NULL,
    room text,
    day date NOT NULL,
    started bigint NOT NULL DEFAULT 0,
    players jsonb NOT NULL DEFAULT '[]',
    mode text,
    instance text,
    status text NOT NULL DEFAULT 'active',
    ended bigint,
    result text
);
CREATE INDEX IF NOT EXISTS game_history_time ON wzq_private.game_history (day DESC, started DESC, id DESC);
CREATE INDEX IF NOT EXISTS game_history_room ON wzq_private.game_history (room, day DESC, started DESC, id DESC);
CREATE TABLE IF NOT EXISTS wzq_private.game_events (
    match_id text REFERENCES wzq_private.game_history(id),
    seq bigint NOT NULL,
    at_ms bigint NOT NULL,
    seat integer,
    action text NOT NULL,
    detail jsonb,
    PRIMARY KEY (match_id, seq)
);

-- Older leaderboard rows contain only an ID and the settlement day.
INSERT INTO wzq_private.game_history(id, game, day, status)
SELECT id, 'gomoku', day, 'legacy' FROM wzq_private.ranked_matches ON CONFLICT DO NOTHING;
DO $$ BEGIN
    IF to_regclass('wzq_private.ddz_matches') IS NOT NULL THEN
        INSERT INTO wzq_private.game_history(id, game, day, status)
        SELECT id, 'ddz', day, 'legacy' FROM wzq_private.ddz_matches ON CONFLICT DO NOTHING;
    END IF;
END $$;

CREATE OR REPLACE FUNCTION wzq_api.save_game_events(p_events jsonb) RETURNS void
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE e jsonb; m jsonb;
BEGIN
    FOR e IN SELECT value FROM jsonb_array_elements(p_events) LOOP
        m := e->'match';
        INSERT INTO wzq_private.game_history(id, game, room, day, started, players, mode, instance)
        VALUES(m->>'id', m->>'game', m->>'room',
            (to_timestamp((m->>'start')::bigint / 1000.0) AT TIME ZONE 'Asia/Shanghai')::date,
            (m->>'start')::bigint, m->'players', m->>'mode', m->>'instance')
        ON CONFLICT (id) DO UPDATE SET room=excluded.room, started=excluded.started, day=excluded.day,
            players=excluded.players, mode=excluded.mode, instance=excluded.instance, status='active'
        WHERE game_history.status='legacy';
        INSERT INTO wzq_private.game_events(match_id, seq, at_ms, seat, action, detail)
        VALUES(m->>'id', (e->>'seq')::bigint, (e->>'at')::bigint, (e->>'seat')::integer, e->>'action', e->'detail')
        ON CONFLICT DO NOTHING;
        IF e->>'status' IS NOT NULL THEN
            UPDATE wzq_private.game_history SET status=e->>'status', result=e->>'action', ended=(e->>'at')::bigint
            WHERE id=m->>'id';
        END IF;
    END LOOP;
END $$;

CREATE OR REPLACE FUNCTION wzq_api.list_game_history(
    p_instance text, p_game text, p_room text, p_day text, p_before text
) RETURNS jsonb LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
    SELECT coalesce(jsonb_agg(to_jsonb(page) ORDER BY day DESC, started DESC, id DESC), '[]')
    FROM (
        SELECT id, game, room, day, started, players, mode, ended, result,
            CASE WHEN status='active' AND instance<>p_instance THEN 'interrupted' ELSE status END AS status
        FROM wzq_private.game_history
        WHERE (p_game IS NULL OR game=p_game) AND (p_room IS NULL OR room=p_room)
            AND (p_day IS NULL OR day=p_day::date)
            AND (p_before IS NULL OR (day,started,id) < (SELECT day,started,id FROM wzq_private.game_history WHERE id=p_before))
        ORDER BY day DESC, started DESC, id DESC LIMIT 26
    ) AS page;
$$;

CREATE OR REPLACE FUNCTION wzq_api.game_history_detail(p_id text, p_instance text)
RETURNS jsonb LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
    SELECT (to_jsonb(m) - 'instance') || jsonb_build_object(
        'status', CASE WHEN status='active' AND instance<>p_instance THEN 'interrupted' ELSE status END,
        'events', (SELECT coalesce(jsonb_agg(to_jsonb(e)-'match_id' ORDER BY seq), '[]')
                   FROM wzq_private.game_events e WHERE match_id=m.id))
    FROM wzq_private.game_history m WHERE id=p_id;
$$;
REVOKE ALL ON FUNCTION wzq_api.save_game_events(jsonb), wzq_api.list_game_history(text,text,text,text,text),
    wzq_api.game_history_detail(text,text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION wzq_api.save_game_events(jsonb), wzq_api.list_game_history(text,text,text,text,text),
    wzq_api.game_history_detail(text,text) TO wzq_app;
NOTIFY pgrst, 'reload schema';
COMMIT;
