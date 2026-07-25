---
name: Feature Request
about: Propose a feature that fits d-lmdb's scope
title: "[Feature] "
labels: "enhancement"
assignees: ""
---

## ⚠️ Read First

d-lmdb targets **fault tolerance, not horizontal scaling** — LMDB stays
single-writer, single-node storage; d-engine only adds replication on top.
Features that turn this into a general-purpose distributed database
(sharding, multi-writer, secondary indexes, etc.) are out of scope — see
[CONTRIBUTING.md](../../CONTRIBUTING.md).

**Before submitting:**

- [ ] This fits the fault-tolerance-not-scaling scope above
- [ ] I've described a real use case, not a hypothetical one

---

## What problem does this solve?

(Be specific: what are you trying to do, and what stops you today?)

## Proposed solution

**Your solution:**

**Why is this the simplest approach?**

## Additional Context

- Similar features in other Raft/LMDB projects (if applicable)
- Your willingness to contribute code
