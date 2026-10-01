# Tournament server

`serve.py` puts a small REST API in front of the `mars` binary, so a remote coding agent can send
Redcode sources and get tournament results back. The agent writes the program variants and keeps
track of the best ones; this server only assembles and plays them. It stores nothing: every
request brings its sources, and they are gone when the reply is sent.

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
| `--max-wars` | 2000000 | wars one request may play, about a minute on one RTX 4090 |

The server speaks plain HTTP. Put it behind a reverse proxy that terminates TLS, and give the
proxy a read timeout that covers `--max-wars` (the default needs about a minute per GPU). One
tournament plays at a time; requests that arrive meanwhile wait their turn.

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
401 (key), 404, 413 (too large: body, program count or wars), 422 (nothing to play), 503 (`/health`
only) or 504.

Program names are 1 to 64 characters of letters, digits, `_`, `.` and `-`, starting with a letter
or digit. A source is at most 64 KB.

### `GET /`

The server's device and limits.

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
{"status": "ok", "busy": false, "checks": {
  "binary": {"ok": true, "seconds": 0.0},
  "device": {"ok": true, "seconds": 0.06},
  "tournament": {"ok": true, "seconds": 0.44}}}
```

`busy` says whether a tournament is playing. The check does not wait for it: its two wars run
next to it. It takes about half a second on a GPU (mostly device startup), so poll it every minute or
so, not every second.

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

| Field | Default | |
|:--|:--|:--|
| `programs` | required | `{name: source}` |
| `against` | none | `{name: source}`: opponents of a gauntlet |
| `games` | 100 | games per pair, half with each first mover |
| `seed` | random | master seed; the same seed repeats the tournament exactly |
| `standard` | `pmars` | `pmars`, `hu93`, `88` or `94` |
| `steps`, `core`, `length`, `distance`, `queue` | of the standard | as the options of `mars` |

Without `against`, every pair of `programs` plays (at most 256 programs). With `against`, every
program plays every opponent and neither group plays among itself (at most 16384 in total). That
is the shape for a GA: new candidates in `programs`, the current best and a fixed reference set
in `against`.

Use a new seed for every evaluation (leave `seed` out). With a fixed seed a search learns the
placements instead of the game.

```bash
curl -s -H "Authorization: Bearer $MARS_API_KEY" -d @request.json http://127.0.0.1:8080/tournaments
```

```json
{
  "seed": 121320931673016, "games": 200, "wars": 200, "seconds": 0.53,
  "errors": {"broken": ["Undefined symbol at line 1 in program broken !"]},
  "standings": [
    {"name": "stone", "games": 200, "wins": 31, "draws": 169, "losses": 0, "score": 0.5775, "points": 131.0},
    {"name": "imp", "games": 200, "wins": 0, "draws": 169, "losses": 31, "score": 0.4225, "points": 84.5}],
  "runs": [{"first": "stone", "second": "imp", "games": 200, "first_wins": 31, "second_wins": 0, "draws": 169, "...": "..."}]
}
```

- `errors`: programs that did not assemble. They are left out and the rest plays. If fewer than
  two programs are left, or no candidate or no opponent, the reply is `422` with the errors.
- `standings`: totals per program, best first; in a gauntlet only the `programs`. `score` is
  (wins + draws / 2) / games, `points` is 3 per win and 1 per draw, per 100 games.
- `runs`: one object per pair, the lines `mars tournament` writes (see the
  [engine's README](../README.md#tournaments)).
