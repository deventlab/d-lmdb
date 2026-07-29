# ============================================================================
# Makefile for d-lmdb
# ============================================================================
# Convenience commands for formatting, linting, testing, and release checks.
#
# Usage: make <target>
# Examples:
#   make help              # Show all available targets
#   make check              # Run all quality checks (fmt, clippy, deny)
#   make fix                # Auto-fix formatting + apply clippy suggestions
#   make test               # Run all tests with nextest
#   make pre-release        # Comprehensive pre-release validation
# ============================================================================

.PHONY: help check-env install-tools install \
        fmt fmt-check clippy clippy-fix check quick-check \
        build build-release test docs docs-check docs-clean \
        audit deny clean clean-deps pre-release fix all

CARGO ?= cargo
RUST_LOG_LEVEL ?= d_lmdb=debug
RUST_BACKTRACE ?= 1

# Keep in sync with .github/workflows/dependency-audit.yml — an unpinned
# cargo-deny version drifts between local and CI and can silently change
# which licenses/advisories pass.
CARGO_DENY_VERSION := 0.20.2

# Keep in sync with .github/workflows/ci.yml — an unpinned cargo-nextest
# grabs the latest release, which can require a newer rustc than the
# toolchain pinned in rust-toolchain.toml (1.89.0).
CARGO_NEXTEST_VERSION := 0.9.114

# docker image
DOCKER_LOCAL_TAG ?= d-lmdb-server:local
DOCKER_REMOTE_TAG ?= deventlab/d-lmdb:latest
COMPOSE_TAG ?= d-lmdb-server:compose

RED := \033[0;31m
GREEN := \033[0;32m
YELLOW := \033[1;33m
BLUE := \033[0;34m
MAGENTA := \033[0;35m
CYAN := \033[0;36m
NC := \033[0m

# ============================================================================
# HELP
# ============================================================================

## Display this help message with all available targets
help:
	@echo "$(BLUE)d-lmdb Build System$(NC)"
	@echo ""
	@echo "$(CYAN)Available Targets:$(NC)"
	@awk '/^## / { desc = substr($$0, 4) } \
	      /^[a-zA-Z_-]+:/ && desc { split($$0, a, ":"); \
	      printf "  $(GREEN)%-16s$(NC) %s\n", a[1], desc; desc="" }' $(MAKEFILE_LIST)
	@echo ""
	@echo "$(CYAN)Common Workflows:$(NC)"
	@echo "  $(YELLOW)Development:$(NC)  make check          # Run before committing"
	@echo "                   make fix            # Auto-fix fmt + clippy"
	@echo "  $(YELLOW)Testing:$(NC)      make test           # nextest, all features"
	@echo "  $(YELLOW)Docs:$(NC)         make docs           # Build + open in browser"
	@echo "  $(YELLOW)Release:$(NC)      make pre-release    # Full pre-release validation"
	@echo "  $(YELLOW)Maintenance:$(NC)  make clean          # Remove build artifacts"

# ============================================================================
# ENVIRONMENT
# ============================================================================

## Verify Rust environment prerequisites are installed
check-env:
	@command -v $(CARGO) >/dev/null 2>&1 || \
		{ echo "$(RED)✗ cargo not found in PATH$(NC)"; exit 1; }
	@command -v rustc >/dev/null 2>&1 || \
		{ echo "$(RED)✗ rustc not found in PATH$(NC)"; exit 1; }
	@echo "$(GREEN)✓ Cargo found: $$($(CARGO) --version)$(NC)"

## Install required Rust components (rustfmt, clippy, nextest, deny)
install-tools: check-env
	@rustup component add rustfmt clippy 2>/dev/null || true
	@if [ "$$(cargo nextest --version 2>/dev/null | awk '{print $$2}')" != "$(CARGO_NEXTEST_VERSION)" ]; then \
		echo "$(YELLOW)Installing cargo-nextest $(CARGO_NEXTEST_VERSION)...$(NC)"; \
		cargo install cargo-nextest --locked --version $(CARGO_NEXTEST_VERSION); \
	fi
	@if [ "$$(cargo deny --version 2>/dev/null | awk '{print $$2}')" != "$(CARGO_DENY_VERSION)" ]; then \
		echo "$(YELLOW)Installing cargo-deny $(CARGO_DENY_VERSION)...$(NC)"; \
		cargo install cargo-deny --locked --version $(CARGO_DENY_VERSION); \
	fi
	@echo "$(GREEN)✓ Components installed$(NC)"

## Alias for install-tools
install: install-tools

# ============================================================================
# CODE QUALITY
# ============================================================================

## Check code formatting without modifying files (dry-run)
fmt-check:
	@$(CARGO) fmt --all -- --check || \
		{ echo "$(RED)✗ Formatting issues found. Run 'make fix'.$(NC)"; exit 1; }
	@echo "$(GREEN)✓ Formatting check passed$(NC)"

## Fix code formatting automatically
fmt: install-tools
	@$(CARGO) fmt --all
	@echo "$(GREEN)✓ Formatting fixed$(NC)"

## Run Clippy linter (fail on warnings)
clippy: install-tools
	@$(CARGO) clippy --all-targets --all-features -- -D warnings || \
		{ echo "$(RED)✗ Clippy warnings found. Run 'make clippy-fix'.$(NC)"; exit 1; }
	@echo "$(GREEN)✓ Clippy check passed$(NC)"

## Automatically apply Clippy suggestions (review changes!)
clippy-fix: install-tools
	@$(CARGO) clippy --all-targets --all-features --fix --allow-dirty --allow-staged
	@echo "$(YELLOW)Re-checking for remaining issues...$(NC)"
	@$(CARGO) clippy --all-targets --all-features -- -D warnings || \
		{ echo "$(YELLOW)Note: some warnings need manual fixes$(NC)"; }
	@echo "$(MAGENTA)IMPORTANT: Review changes before committing!$(NC)"

## Run all quality checks (fmt, clippy, deny)
check: fmt-check clippy deny
	@echo "$(GREEN)✓ All quality checks passed$(NC)"

## Fast validation (cargo check + fmt-check only)
quick-check:
	@$(CARGO) check --all-targets --all-features
	@$(CARGO) fmt --all -- --check || exit 1
	@echo "$(GREEN)✓ Quick validation passed$(NC)"

# ============================================================================
# BUILD
# ============================================================================

## Build in debug mode
build:
	@$(CARGO) build --all-features
	@echo "$(GREEN)✓ Debug build completed$(NC)"

## Build in release mode (optimized)
build-release: check
	@$(CARGO) build --all-features --release
	@echo "$(GREEN)✓ Release build completed$(NC)"

# ============================================================================
# TESTING
# ============================================================================

## Run all tests with nextest (fast, parallel)
test: check install-tools docker-cve-check
	@CI=1 RUST_LOG=$(RUST_LOG_LEVEL) RUST_BACKTRACE=$(RUST_BACKTRACE) \
		$(CARGO) nextest run --all-features --no-fail-fast
	@$(CARGO) test --doc --all-features
	@echo "$(BLUE)Verifying examples compile...$(NC)"
	@$(CARGO) check --examples --all-features
	@echo "$(GREEN)✓ All tests passed$(NC)"

# ============================================================================
# DOCUMENTATION
# ============================================================================

## Generate API documentation and open in browser
docs:
	@$(CARGO) doc --all-features --no-deps
	@echo "$(GREEN)✓ Documentation generated$(NC)"
	@open "file://$$(pwd)/target/doc/d_lmdb/index.html" 2>/dev/null || \
		xdg-open "file://$$(pwd)/target/doc/d_lmdb/index.html" 2>/dev/null || true

## Check documentation with strict warnings (simulates docs.rs)
docs-check:
	@RUSTDOCFLAGS="-D warnings" $(CARGO) doc --all-features --no-deps
	@echo "$(GREEN)✓ Documentation compiled without warnings$(NC)"

## Remove generated documentation artifacts
docs-clean:
	@rm -rf target/doc
	@echo "$(GREEN)✓ Documentation cleaned$(NC)"

# ============================================================================
# SECURITY & DEPENDENCY AUDITING
# ============================================================================

## Audit dependencies for known security vulnerabilities
audit:
	@if command -v cargo-audit >/dev/null 2>&1; then \
		cargo audit; \
	else \
		echo "$(YELLOW)cargo-audit not installed. Install with: cargo install cargo-audit$(NC)"; \
	fi

## Run cargo-deny for supply chain security checks
deny:
	@if command -v cargo-deny >/dev/null 2>&1; then \
		cargo deny check all; \
	else \
		echo "$(YELLOW)cargo-deny not installed. Run: make install-tools$(NC)"; \
	fi

# ============================================================================
# CLEANUP
# ============================================================================

## Remove all build artifacts
clean:
	@$(CARGO) clean
	@rm -rf coverage/ *.log
	@echo "$(GREEN)✓ Cleanup completed$(NC)"

## Aggressive cleanup: artifacts + Cargo.lock
clean-deps: clean
	@rm -f Cargo.lock
	@echo "$(GREEN)✓ Deep cleanup completed$(NC)"

# ============================================================================
# COMPOSITE WORKFLOWS
# ============================================================================

## Run automatic fixes (fmt + clippy-fix)
fix: fmt clippy-fix
	@echo "$(GREEN)✓ Automatic fixes completed$(NC)"
	@echo "$(MAGENTA)IMPORTANT: Review and test your changes before committing!$(NC)"

## Full pre-release validation (comprehensive checks)
pre-release: install-tools check docs-check test audit build-release
	@echo ""
	@echo "$(GREEN)╔═══════════════════════════════════════════════════════╗$(NC)"
	@echo "$(GREEN)║  ✓ ALL PRE-RELEASE CHECKS PASSED SUCCESSFULLY!       ║$(NC)"
	@echo "$(GREEN)╚═══════════════════════════════════════════════════════╝$(NC)"
	@echo ""
	@echo "$(MAGENTA)Next Steps:$(NC)"
	@echo "  1. Review NOTICES / CHANGELOG for accuracy"
	@echo "  2. Bump version in Cargo.toml (if needed)"
	@echo "  3. Create annotated git tag: git tag -a v<VERSION>"
	@echo "  4. Push: git push && git push --tags"

# ============================================================================
# DOCKER VERIFICATION
# ============================================================================

## Build local image + single-node + 3-node HA smoke test
docker-verify-local: docker-verify-build
	@docker tag $(DOCKER_LOCAL_TAG) $(COMPOSE_TAG)
	@DLMDB_SKIP_BUILD=1 ./compose/smoke-test.sh
	@echo "✓ Docker image verified (local)"

## Pull remote image + single-node + 3-node HA smoke test
docker-verify-remote: docker-verify-pull
	@docker tag $(DOCKER_REMOTE_TAG) $(COMPOSE_TAG)
	@DLMDB_SKIP_BUILD=1 ./compose/smoke-test.sh
	@echo "✓ Docker image verified (remote)"

# Internal: build local + single-node test
docker-verify-build:
	@docker info >/dev/null 2>&1 || { echo "WARN: Docker not available — skipping"; exit 0; }; \
	docker build -t $(DOCKER_LOCAL_TAG) . && \
	docker rm -f dlmdb-verify 2>/dev/null || true; \
	docker volume rm dlmdb-verify-data 2>/dev/null || true; \
	docker run -d --name dlmdb-verify -p 18080:8080 \
		-e CONFIG=/etc/dlmdb/config.toml \
		-v $(PWD)/d-lmdb-server/config.example.toml:/etc/dlmdb/config.toml:ro \
		-v dlmdb-verify-data:/data $(DOCKER_LOCAL_TAG) && \
	sleep 3 && \
	curl -s -o /dev/null -w 'PUT: %{http_code}\n' -X PUT localhost:18080/kv/hello -d world && \
	curl -s -w '\n' localhost:18080/kv/hello; \
	docker rm -f dlmdb-verify; \
	docker volume rm dlmdb-verify-data

# Internal: pull remote + single-node test
docker-verify-pull:
	@docker info >/dev/null 2>&1 || { echo "WARN: Docker not available — skipping"; exit 0; }; \
	docker pull $(DOCKER_REMOTE_TAG) && \
	docker rm -f dlmdb-verify 2>/dev/null || true; \
	docker volume rm dlmdb-verify-data 2>/dev/null || true; \
	docker run -d --name dlmdb-verify -p 18080:8080 \
		-e CONFIG=/etc/dlmdb/config.toml \
		-v $(PWD)/d-lmdb-server/config.example.toml:/etc/dlmdb/config.toml:ro \
		-v dlmdb-verify-data:/data $(DOCKER_REMOTE_TAG) && \
	sleep 3 && \
	curl -s -o /dev/null -w 'PUT: %{http_code}\n' -X PUT localhost:18080/kv/hello -d world && \
	curl -s -w '\n' localhost:18080/kv/hello; \
	docker rm -f dlmdb-verify; \
	docker volume rm dlmdb-verify-data


## Build local image + fail if Medium+ CVEs exist
docker-cve-check: docker-verify-build
	@docker info >/dev/null 2>&1 || { echo "WARN: Docker not available — skipping"; exit 0; }; \
	command -v docker >/dev/null 2>&1 || { echo "docker scout CLI required"; exit 1; }; \
	ARCH=$$(uname -m | sed 's/x86_64/amd64/;s/aarch64/arm64/'); \
	OUT=$$(docker scout cves $(DOCKER_LOCAL_TAG) --platform linux/$$ARCH 2>&1); \
	LINE=$$(echo "$$OUT" | grep 'vulnerabilities' || true); \
	[ -n "$$LINE" ] || { echo "$$OUT"; exit 1; }; \
	C=$$(echo "$$LINE" | grep -o '[0-9]\+C' | grep -o '[0-9]\+' || echo 0); \
	H=$$(echo "$$LINE" | grep -o '[0-9]\+H' | grep -o '[0-9]\+' || echo 0); \
	M=$$(echo "$$LINE" | grep -o '[0-9]\+M' | grep -o '[0-9]\+' || echo 0); \
	echo "CVEs: $${C}C $${H}H $${M}M"; \
	if [ "$$C" -gt 0 ] || [ "$$H" -gt 0 ] || [ "$$M" -gt 0 ]; then \
		echo "FAIL: Medium+ vulnerabilities found"; exit 1; \
	fi; \
	echo "PASS"

## Default target: run full check suite
all: check
	@echo "$(GREEN)✓ Check suite completed$(NC)"

.DEFAULT_GOAL := help
