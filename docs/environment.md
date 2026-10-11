# Runtime environment variables

Two variables change how the server runs. Both are read once, at process start —
neither can be changed while the server is running.

| Variable | Default | What it does |
|----------|---------|--------------|
| `GOL_PORT` | `7654` | TCP port for the web UI and the `/state` API |
| `GOL_WALK_THREADS` | `7` | Worker threads used by the engine's per-gc cache walk |

## GOL_PORT

The port the server listens on. It binds `0.0.0.0`, so the UI and the API are
reachable from other machines on the network.

The common use is running a second instance beside the production one — for
experiments, a benchmark run, or comparing two builds:

```sh
GOL_PORT=7698 ./target/release/game_of_life
```

Then open `http://localhost:7698`, or drive it directly:

```sh
curl -s "http://localhost:7698/state?vx=0&vy=0&vw=530&vh=300&scale=1" -o frame.bin
```

Notes:

- A value that does not parse as a port number is ignored; the default `7654` is
  used instead.
- If the port is already in use, the process exits with `Failed to bind`.
- There is no command-line option for the port, and no pattern file argument —
  patterns are loaded through the UI or `POST /load-pattern`.

## GOL_WALK_THREADS

Number of worker queues the HashLife cache walk uses when it walks the tree to
recount live cells (it runs on each cache collection, not every step).

```sh
GOL_WALK_THREADS=4 ./target/release/game_of_life
```

Notes:

- Read once and cached, so it must be set before the process starts.
- Must be at least 1; `0` or an unparseable value falls back to the default `7`.
- The useful range is roughly 4–8 on an 8-core machine; the walk's speedup
  plateaus around 4 workers. Raising it does not cost memory — the queues are
  sized `arena/nt` each, so total capacity is about the arena size either way —
  it just splits the walk into more, smaller tasks.
- The walk runs on cache collection, not on every generation step, so this is
  not a general "threads used for stepping" control.
