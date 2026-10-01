# Tournament server

`serve.py` puts a small REST API in front of the `mars` binary, so a remote coding agent can send
Redcode sources and get tournament results back. The agent writes the program variants and keeps
track of the best ones; this server only assembles and plays them.

A tournament is submitted, gets an ID, and waits in a queue that the server works down one at a
time. The agent polls or long polls for the result, sees the progress meanwhile, can cancel, and
can list its tournaments to catch up after a restart. The server keeps all of this in memory:
finished tournaments are forgotten after an hour, and everything is gone when the server
restarts.

It does not serve the benchmark field. Those warriors are played only by `breed.py`, which logs
each use.

## Running it

```bash
cargo build --release                       # in Modern
MARS_API_KEY=$(openssl rand -hex 24) python3 Breeding/serve.py --port 8080
```

| Option | Default | |
|:--|:--|:--|
| `MARS_API_KEY` | required | the pre-shared key, at least 16 characters |
| `--host` | 127.0.0.1 | address to listen on |
| `--port` | 8080 | |
| `--gpu` | every discrete GPU, else the CPU | `1`, `0,1` or `cpu`; also `BREED_GPUS`. Never an integrated or software GPU |
| `--max-queued` | 10 | tournaments that may wait in the queue; one more is refused with `429` |
| `--max-wars` | 20000000 | wars one tournament may play, about ten minutes on one RTX 4090 |
| `--timeout` | 60 | minutes after which a running tournament is stopped as failed |
| `--keep` | 60 | minutes a finished tournament and its results stay available |

The server speaks plain HTTP. Put it behind a reverse proxy that terminates TLS. If the agent long
polls, the proxy's read timeout has to be longer than the `wait` it uses (at most 300 seconds).

## Authentication

Every request except `GET /livez` needs the key, in either form:

```
Authorization: Bearer KEY
Authorization: Basic base64(anything:KEY)
```

Without it the answer is `401`. With `curl`: `-H "Authorization: Bearer $MARS_API_KEY"` or
`-u agent:$MARS_API_KEY`.

## API

Bodies and replies are JSON. An error reply is `{"error": "..."}` with status 400 (bad request),
401 (key), 404, 413 (too large: body, program count or wars), 422 (nothing to play), 429 (queue
full), 503 (`/health` only) or 504.

A source is at most 64 KB and a request body at most 16 MB.

### `GET /`

The server's device and limits (queue size, wars per tournament, longest `wait`, and so on).

### `GET /livez`

Cheap liveness check: `{"status": "ok"}` as soon as the process answers. It needs no key and
touches neither the binary nor the GPUs, so a proxy or a supervisor can poll it often.

### `GET /health`

Extensive check of the whole path a tournament takes, with the key. It answers `200` when every
check passes and `503` otherwise, with the failed check's `error`:

- `binary`: the `mars` binary is there.
- `device`: the configured discrete GPUs are still listed by `mars gpus`.
- `tournament`: two probe programs assemble, and a two-war tournament on the configured device
  ends with the known winner.

```json
{"status": "ok", "busy": false, "queued": 0, "running": 0, "checks": {
  "binary": {"ok": true, "seconds": 0.0},
  "device": {"ok": true, "seconds": 0.06},
  "tournament": {"ok": true, "seconds": 0.44}}}
```

`busy` says whether a tournament is playing, and `queued` and `running` count them. The check
does not go through the queue: its two wars run next to a playing tournament. It takes about half
a second on a GPU (mostly device startup), so poll it every minute or so, not every second.

### `POST /compile`

```json
{"programs": {"stone": "ADD.AB #3044, 1\nMOV.I 2, 2\nJMP -2\nDAT #0, #0\n", "broken": "MOV 0, nowhere\n"}}
```

```json
{"programs": {
  "stone": {"ok": true, "start": 0, "instructions": [{"op": "ADD", "modifier": "AB", "a_mode": "#", "a": 3044, "b_mode": "$", "b": 1}]},
  "broken": {"ok": false, "errors": ["Undefined symbol at line 1 in program broken !"]}}}
```

### `POST /tournaments`

Queues a tournament and answers with its state, `202` while it is queued or running.

| Field | Default | |
|:--|:--|:--|
| `programs` | required | `{id: source}` |
| `against` | none | `{id: source}`: opponents of a gauntlet |
| `games` | 100 | games per pair, half with each first mover |
| `seed` | random | master seed; the same seed repeats the tournament exactly |
| `label` | empty | free text of at most 200 characters, returned with the tournament |
| `standard` | `pmars` | `pmars`, `hu93`, `88` or `94` |
| `steps`, `core`, `length`, `distance`, `queue` | of the standard | as the options of `mars` |

A program ID is any string of 1 to 200 characters: a name, a UUID, a hash of the source. It only
has to be unique within the request. Results refer to programs by these IDs, and the sources come
back with them, so a result can be matched to exactly what was sent.

Without `against`, every pair of `programs` plays (at most 256 programs). With `against`, every
program plays every opponent and neither group plays among itself (at most 16384 in total). That
is the shape for a GA: new candidates in `programs`, the current best and a fixed reference set
in `against`.

Use a new seed for every evaluation (leave `seed` out). With a fixed seed a search learns the
placements instead of the game.

Programs that do not assemble are left out, reported under `errors`, and the rest plays. The
request is refused right away, with nothing queued, when it is malformed (`400`), too large
(`413`), leaves fewer than two programs or no candidate or opponent to play (`422`, with the
`errors`), or the queue is full (`429`).

```bash
curl -s -H "Authorization: Bearer $MARS_API_KEY" -d @request.json http://127.0.0.1:8080/tournaments
```

```json
{"id": "ed427ce3-be9e-4d62-bc04-7f195cdf2a24", "label": "generation 7", "status": "queued", "position": 2,
 "progress": 0.0, "wars_done": 0, "wars": 360000, "games": 60000, "seed": 58404828520735,
 "created": "2026-10-01T09:01:16+00:00", "started": null, "finished": null, "errors": {},
 "programs": {"stone": "...", "imp": "..."}, "against": {}}
```

### `GET /tournaments/ID`

The same object, with the current state:

| `status` | |
|:--|:--|
| `queued` | waiting; `position` is its place in the queue, 1 is next |
| `running` | playing; `progress` is the percentage of wars done |
| `done` | finished; `seconds`, `standings` and `runs` are there |
| `failed` | `error` says why (for example the timeout) |
| `canceled` | canceled with `DELETE` |

The answer is `202` for `queued` and `running` and `200` once the tournament is over. `progress`
and `wars_done` come from the engine's own progress output, which it writes every five seconds, so
a tournament shorter than that goes from 0 to 100.

A finished tournament:

```json
{"id": "...", "label": "generation 7", "status": "done", "progress": 100.0, "wars_done": 200, "wars": 200,
 "games": 200, "seed": 121320931673016, "created": "...", "started": "...", "finished": "...", "seconds": 0.53,
 "errors": {"broken": ["Undefined symbol at line 1 in program broken !"]},
 "standings": [
   {"id": "stone", "games": 200, "wins": 31, "draws": 169, "losses": 0, "score": 0.5775, "points": 131.0},
   {"id": "imp", "games": 200, "wins": 0, "draws": 169, "losses": 31, "score": 0.4225, "points": 84.5}],
 "runs": [{"first": "stone", "second": "imp", "games": 200, "first_wins": 31, "second_wins": 0, "draws": 169, "...": "..."}],
 "programs": {"stone": "ADD.AB #3044, 1\n...", "imp": "MOV 0, 1\n", "broken": "MOV 0, nowhere\n"}, "against": {}}
```

- `standings`: totals per program, best first; in a gauntlet only the `programs`. `score` is
  (wins + draws / 2) / games, `points` is 3 per win and 1 per draw, per 100 games.
- `runs`: one object per pair, the lines `mars tournament` writes (see the
  [engine's README](../README.md#tournaments)), with the program IDs as `first` and `second`.
- `programs` and `against`: the sources as they were sent.

### Long polling: `?wait=SECONDS`

On `GET /tournaments/ID` and on `POST /tournaments`, `?wait=30` holds the answer until the
tournament is over or 30 seconds have passed, whichever comes first (at most 300). A loop of
`GET /tournaments/ID?wait=30` gets the result as soon as it exists and a progress update every 30
seconds until then. `POST /tournaments?wait=60` gives a short tournament's result in one call.

### `GET /tournaments`

`{"tournaments": [...]}`: every queued, running and recently finished tournament in submission
order, as above but without `standings`, `runs` and the sources. An agent that was restarted
finds its work here, by `id` or by the `label` it gave, and then fetches each result.

### `DELETE /tournaments/ID`

Cancels a queued or running tournament; a running one stops within moments and the next in the
queue starts. Its `status` becomes `canceled` and it stays listed like a finished one. Deleting a
tournament that is already over forgets it and frees its results.
