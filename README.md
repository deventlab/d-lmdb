# d-lmdb

![status](https://img.shields.io/badge/status-experimental-orange)
[![CI](https://github.com/deventlab/d-lmdb/actions/workflows/ci.yml/badge.svg)](https://github.com/deventlab/d-lmdb/actions/workflows/ci.yml)
![Static Badge](https://img.shields.io/badge/license-MIT%20%7C%20Apache--2.0-blue)

d-lmdb demonstrates one concrete way [d-engine](https://github.com/DEventLab/d-engine) (a Raft consensus library) can add distributed replication to an existing embedded storage engine — LMDB was chosen because it's a well-known, compact embedded key-value store. The broader point: **d-engine can pair with many storage engines and applications to add an optional, replicated write path** — LMDB is just this example.

This repo ships the same idea two ways — pick one:

## Choose your path

| | `d-lmdb` (library) | `d-lmdb-server` (service) |
|---|---|---|
| You are | writing Rust, embedding the store in your process | running any language, want a standalone service |
| You get | in-process calls, no network hop | HTTP+JSON over the network, `docker run` |
| Trade-off | fastest reads/writes, but every replica embeds a Raft node | one network hop, but replicas are decoupled from your app |

→ Rust, embedded: [`d-lmdb/README.md`](./d-lmdb/README.md)
→ HTTP, Docker: [`d-lmdb-server/README.md`](./d-lmdb-server/README.md)

## What this solves

- **Single point of failure → quorum fault tolerance.** Writes go through Raft; the cluster keeps serving as long as a majority of nodes are up.
- **Cross-machine strong consistency, one extra dependency.** Multiple nodes see the same linearizable write history — no separate coordination service to run.
- **Reads stay fast.** Reads bypass Raft and hit local storage directly, in both the embedded and HTTP form.

## What this doesn't solve

Stated plainly, so expectations are set correctly:

- **Write throughput does not improve — it gets worse.** A local write is one fsync; a replicated write is a network round trip plus a quorum of fsyncs. This trades latency/throughput for availability and consistency.
- **No sharding.** d-engine replicates data, it does not partition it — every node holds the full dataset.
- **Still just a key-value store.** No relational modeling or complex queries.
- **Raft adds real operational surface.** Leader election, membership changes, snapshots, log compaction — none of this exists with plain LMDB.

## Repository layout

- [`d-lmdb/`](./d-lmdb) — the Rust library
- [`d-lmdb-server/`](./d-lmdb-server) — the HTTP server binary + Docker image
- [`compose/`](./compose) — 3-node HA cluster reference (HAProxy + smoke test)
- [`examples/`](./examples) — runnable single-node / three-node examples using the library directly

## Project Status

Early-stage and evolving — intended for evaluation, experimentation, and community feedback. Not yet extensively validated in production. Issues, discussions, forks, and PRs are welcome.

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT License](LICENSE-MIT) at your option. Built on [d-engine](https://github.com/DEventLab/d-engine) (Apache-2.0 / MIT) and [LMDB](https://www.symas.com/lmdb) (OpenLDAP Public License) — see [NOTICES](NOTICES).
