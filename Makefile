.PHONY: cov clean report html

TIMESTAMP := $(shell date +%Y%m%d-%s)
PROF := profile-$(TIMESTAMP)
PROFDATA := cov/$(PROF).profdata
TESTBIN := $(shell ./scripts/list_test_bin.sh)

cov: cov/index.html report

report: $(PROFDATA)
	llvm-cov report --use-color --ignore-filename-regex='/.cargo/registry' --instr-profile=$(PROFDATA) $(TESTBIN)

cov/index.html: $(PROFDATA)
	llvm-cov show --format=html --use-color --ignore-filename-regex='/.cargo/registry' --instr-profile=$(PROFDATA) $(TESTBIN) > $@

%.profdata: %.profraw
	llvm-profdata merge -sparse $< -o $@

cov/$(PROF).profraw:
	LLVM_PROFILE_FILE="$@" RUSTFLAGS="-C instrument-coverage" cargo test

clean:
	rm -rf cov/*
