# Contributing to d-lmdb

Thank you for your interest in d-lmdb!

## Scope

d-lmdb shows how d-engine (Raft) turns an embedded LMDB store into a
replicated one. It targets **fault tolerance, not horizontal scaling**
— LMDB stays single-writer, single-node storage; d-engine only adds
replication on top.

Contributions should stay within that scope. Features that turn this
into a general-purpose distributed database (sharding, multi-writer,
secondary indexes, etc.) are out of scope — that's a different project.

## Before You Contribute

**Feature ideas**: open an issue first and describe the real use case.
**Bug fixes**: PRs welcome directly, with a reproduction test.

## Development Workflow

```bash
git clone https://github.com/deventlab/d-lmdb.git
cd d-lmdb
make test  # cargo nextest, all features
```

- Branch naming: `feature/<short-description>`, `bug/<short-description>`
- Target `main` for all PRs
- Rebase (don't merge) to keep your branch current

### Code Quality

- `make check` (fmt + clippy + deny) must pass
- `make test` must pass
- New behavior needs tests; TDD preferred

## Pull Request Guidelines

- [ ] Feature discussed in an issue first (if applicable)
- [ ] `make check` and `make test` pass locally
- [ ] Tests added for new/changed behavior
- [ ] Docs updated if public API changed

Small, focused PRs get reviewed faster. Large PRs (> 300 lines) may be
asked to split.

## Review Process

Single-maintainer project — please be patient. Small PRs are reviewed
faster than large ones.

## License

By contributing, you agree to license your work under:

- [MIT License](LICENSE-MIT) or [Apache License 2.0](LICENSE-APACHE)
