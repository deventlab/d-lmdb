# Three-node d-lmdb cluster example.
#
# Each node must run in its own terminal. Start all three before any writes.
# The cluster requires 2-of-3 quorum — one node may be killed and the cluster
# stays operational.
#
# Usage:
#   make build          — compile the example binary
#   make n1             — run node 1 (port 9081)
#   make n2             — run node 2 (port 9082)
#   make n3             — run node 3 (port 9083)
#   make start-cluster  — run all three nodes in parallel
#   make clean          — remove all node data and logs

CARGO      := cargo
CRATE_ROOT := ../..
EXAMPLE    := three_node
CONFIG_DIR := examples/three-node/config
RUST_LOG   ?= d_engine=warn,d_lmdb=info

.PHONY: build n1 n2 n3 start-cluster clean

build:
	cd $(CRATE_ROOT) && $(CARGO) build --example $(EXAMPLE)

n1:
	cd $(CRATE_ROOT) && RUST_LOG=$(RUST_LOG) CONFIG=$(CONFIG_DIR)/n1.toml $(CARGO) run --example $(EXAMPLE)

n2:
	cd $(CRATE_ROOT) && RUST_LOG=$(RUST_LOG) CONFIG=$(CONFIG_DIR)/n2.toml $(CARGO) run --example $(EXAMPLE)

n3:
	cd $(CRATE_ROOT) && RUST_LOG=$(RUST_LOG) CONFIG=$(CONFIG_DIR)/n3.toml $(CARGO) run --example $(EXAMPLE)

start-cluster:
	@echo "Starting 3-node cluster in parallel..."
	$(MAKE) -j3 n1 n2 n3

# Remove all node data directories and logs created at the crate root.
clean:
	rm -rf $(CRATE_ROOT)/db $(CRATE_ROOT)/logs
