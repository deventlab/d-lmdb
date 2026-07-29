# d-lmdb

![status](https://img.shields.io/badge/status-experimental-orange)
[![CI](https://github.com/deventlab/d-lmdb/actions/workflows/ci.yml/badge.svg)](https://github.com/deventlab/d-lmdb/actions/workflows/ci.yml)
[![codecov](https://codecov.io/gh/deventlab/d-lmdb/graph/badge.svg)](https://codecov.io/gh/deventlab/d-lmdb)
[![Docker Pulls](https://img.shields.io/docker/pulls/deventlab/d-lmdb)](https://hub.docker.com/r/deventlab/d-lmdb)
![Static Badge](https://img.shields.io/badge/license-MIT%20%7C%20Apache--2.0-blue)
[![Ask DeepWiki](https://deepwiki.com/badge.svg)](https://deepwiki.com/deventlab/d-lmdb)

## What This Is

d-lmdb demonstrates [d-engine](https://github.com/DEventLab/d-engine)'s core capability: adding distributed, strongly-consistent replication to an existing embedded storage engine.

The broader point: **d-engine adds an optional replicated write path to storage engines and applications.** — LMDB is one possible example.

## Choose your path

|           | `d-lmdb` (library)                                       | `d-lmdb-server` (service)                         |
| --------- | -------------------------------------------------------- | ------------------------------------------------- |
| You are   | writing Rust, embedding the store in your process        | running any language, want a standalone service   |
| You get   | in-process calls, no HTTP hop                            | HTTP+JSON over the network, standalone deployment |
| Trade-off | replicated writes add a network round-trip vs plain LMDB | one extra HTTP hop vs embedded `d-lmdb`           |

→ Rust, embedded: [`d-lmdb/README.md`](./d-lmdb/README.md)  
→ HTTP, standalone: [`d-lmdb-server/README.md`](./d-lmdb-server/README.md)

---

## What this solves

- **Replication → High Availability.** d-engine adds an optional replicated write path, turning a single-node embedded store into a quorum-tolerant system.

## What this doesn't solve

- **Write throughput will not improve.** A local LMDB write is one fsync; a d-lmdb write pays a network round-trip plus a quorum fsync — this trades latency and throughput for availability and consistency.
- **No sharding.** d-engine replicates data, it doesn't partition it — every node stores the full dataset. If your data doesn't fit on one node's LMDB, d-lmdb won't help.
- **LMDB's own constraints are untouched.** d-lmdb wraps LMDB, it doesn't fix it — map-size limits, single-writer-per-node, and long-lived-reader map growth are all still there.

## Repository layout

- [`d-lmdb/`](./d-lmdb) — the Rust library
- [`d-lmdb-server/`](./d-lmdb-server) — the HTTP server binary + Docker image
- [`compose/`](./compose) — 3-node HA cluster reference (HAProxy + smoke test)
- [`examples/`](./examples) — runnable single-node / three-node examples using the library directly

## Project Status

Early-stage and evolving — intended for evaluation, experimentation, and community feedback. Not yet extensively validated in production. Issues, discussions, forks, and PRs are welcome.

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT License](LICENSE-MIT) at your option. Built on [d-engine](https://github.com/DEventLab/d-engine) (Apache-2.0 / MIT) and [LMDB](https://www.symas.com/lmdb) (OpenLDAP Public License) — see [NOTICES](NOTICES).
