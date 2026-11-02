PYTHON ?= python3
BENCH := $(PYTHON) bench.py

.PHONY: selftest list identity run gate

selftest:
	$(BENCH) selftest

list:
	$(BENCH) list

# Requires SPKBENCH_ORACLE_PYTHON or --oracle-python for the run target.
identity:
	$(BENCH) identity

run:
	$(BENCH) run --reps 5 --max-seconds 900
