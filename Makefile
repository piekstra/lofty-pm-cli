# lofty-pm — task runner: build/test/lint/fmt/clean/install, a cheap `smoke`
# check, and an aggregate `verify` gate. Thin wrappers over cargo so a green
# local run predicts a green CI run.

BIN := lofty-pm
CARGO := cargo
# CI builds, lints and tests with `--locked`, so these targets do too: a stale
# Cargo.lock fails here as it would in CI instead of being re-resolved. After
# editing Cargo.toml, refresh the lockfile once with `make deps`.
CARGO_FLAGS := --locked

.PHONY: all build release test lint fmt fmt-check clean install deps smoke verify dev

all: verify

build: SIGN_TARGET = target/debug/$(BIN)
build:
	$(CARGO) build $(CARGO_FLAGS)
	@$(SIGN)

release: SIGN_TARGET = target/release/$(BIN)
release:
	$(CARGO) build --release $(CARGO_FLAGS)
	@$(SIGN)

test:
	$(CARGO) test --all $(CARGO_FLAGS)

lint:
	$(CARGO) clippy --all-targets $(CARGO_FLAGS) -- -D warnings

fmt:
	$(CARGO) fmt --all

fmt-check:
	$(CARGO) fmt --all -- --check

clean:
	$(CARGO) clean

# `cargo install` ad-hoc signs, which gives the binary a *new* code identity
# every time. macOS scopes keychain "Always Allow" grants to that identity, so
# an unsigned reinstall silently revokes them and the next run prompts again.
# Re-signing with the stable shared identity keeps one grant valid across every
# future install.
#
# `--locked` builds from Cargo.lock. Without it `cargo install` ignores the
# lockfile and resolves the newest semver-compatible crates, which are not the
# versions the tests and `cargo audit` ran against.
install: SIGN_TARGET = $${CARGO_INSTALL_ROOT:-$$HOME/.cargo}/bin/$(BIN)
install:
	$(CARGO) install --path . --locked --force
	@$(SIGN)

deps:
	$(CARGO) fetch

# Cheap sanity checks needing no config or network: version + top-level help.
smoke: release
	./target/release/$(BIN) --version
	./target/release/$(BIN) --help > /dev/null
	./target/release/$(BIN) info > /dev/null
	./target/release/$(BIN) mcp --list-tools > /dev/null

verify: fmt-check lint test smoke

# The family's name for the signed debug build; same as `build`.
dev: build

# Shared re-signing step. No-ops with a note when the helper or identity is
# absent (CI, Linux, a fresh machine that hasn't run setup-dev-signing.sh).
define SIGN
if [ -x "$$HOME/Dev/cli-common/scripts/dev-sign.sh" ]; then \
	"$$HOME/Dev/cli-common/scripts/dev-sign.sh" "$(SIGN_TARGET)"; \
else echo "cli-common/scripts/dev-sign.sh not found — $(SIGN_TARGET) left ad-hoc signed"; fi
endef
