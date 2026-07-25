## What Does This PR Do?

(1-2 sentence summary)

**Type:**

- [ ] Bug Fix (with test)
- [ ] Feature (issue #\_\_\_ discussed first)
- [ ] Documentation
- [ ] Test/Coverage
- [ ] Performance (with benchmark)

---

## Why Is This Needed?

**For features:** Link to the issue where this was discussed: #\_\_\_

**For bugs:** Describe the problem and how this fixes it

---

## Checklist

**Required:**

- [ ] `make check` (fmt + clippy + deny) passes
- [ ] `make test` passes
- [ ] Added tests for new code
- [ ] Commits squashed to 1-2 logical units

**If changing APIs:**

- [ ] Updated relevant docs
- [ ] Explained why complexity is justified

---

## Testing

**How tested:**

- Unit tests: (describe)
- Integration tests: (if applicable)
- Manual testing: (if applicable)

**For bug fixes:**

- [ ] Added test that fails without this fix

---

## Does This Fit d-lmdb's Scope?

- [ ] Stays within fault-tolerance, not scaling (no sharding/multi-writer/etc.)
- [ ] Keeps implementation simple
- [ ] Doesn't bloat the public API surface

---

## Reviewer Notes

(Optional: anything reviewers should focus on)

**Estimated review complexity:**

- [ ] Quick (< 100 lines)
- [ ] Medium (< 300 lines)
- [ ] Deep (> 300 lines)
