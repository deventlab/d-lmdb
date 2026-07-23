# Single-node d-lmdb example.
#
# Usage:
#   make build   — compile the example binary
#   make run     — build and run (data written to ../../data/)
#   make clean   — remove the data directory

CARGO      := cargo
CRATE_ROOT := ../..
EXAMPLE    := single_node

.PHONY: build run clean

build:
	cd $(CRATE_ROOT) && $(CARGO) build --example $(EXAMPLE)

run:
	cd $(CRATE_ROOT) && $(CARGO) run --example $(EXAMPLE)

# Remove data directory created at the crate root by the example.
clean:
	rm -rf $(CRATE_ROOT)/data
