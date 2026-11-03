PYTHON ?= python3
BENCH := $(PYTHON) bench.py
REPO := $(abspath ../..)
ORACLE ?= $(REPO)/.venv-rust-oracle/bin/python
INSPECTION ?= $(REPO)/target/release/spec-kitty-rust
EVIDENCE ?=

.PHONY: selftest registry list identity fixtures build run gate conformance

selftest:
	$(BENCH) selftest

registry:
	$(BENCH) registry

list:
	$(BENCH) list

identity:
	$(BENCH) identity --oracle-python $(ORACLE) --rust-binary $(INSPECTION)

fixtures:
	$(BENCH) fixtures verify

# Writes target/release/build-receipt.json binding source -> binary.
# Not run by default; certification requires an explicit build.
build:
	CARGO_BUILD_JOBS=$${CARGO_BUILD_JOBS:-2} CARGO_INCREMENTAL=0 \
		$(BENCH) build --receipt $(REPO)/target/release/build-receipt.json

# Untimed mutation conformance (no ratio is claimed).
conformance:
	PYTHONPATH=$(REPO)/src SPEC_KITTY_PACKS_ROOT=$(REPO)/packs \
		$(ORACLE) tools/mutation_check.py \
		--rust-binary $(REPO)/target/release/spec-kitty \
		--oracle-python $(ORACLE) --packs-root $(REPO)/packs --invocations 2

run:
	$(BENCH) run --oracle-python $(ORACLE) --rust-binary $(INSPECTION) \
		--build-receipt $(REPO)/target/release/build-receipt.json --reps 20

gate:
	@test -n "$(EVIDENCE)" || (echo "usage: make gate EVIDENCE=benchmarks/rust/evidence/<run>" >&2; exit 2)
	$(BENCH) gate $(EVIDENCE) --oracle-python $(ORACLE) --rust-binary $(INSPECTION)
