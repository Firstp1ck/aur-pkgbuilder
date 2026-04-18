# Makefile for aur-pkgbuilder
# Invoked from the repo root via the wrapper Makefile (`make <target>`).
# Or: `make -C dev <target>`

.PHONY: help fmt clippy check test test-ignored build release clean
.PHONY: pre-commit all deny gitleaks
.PHONY: cognitive errors docs
.PHONY: clean-all install

.DEFAULT_GOAL := help

COLOR_RESET := $(shell tput sgr0)
COLOR_BOLD := $(shell tput bold)
COLOR_GREEN := $(shell tput setaf 2)
COLOR_YELLOW := $(shell tput setaf 3)
COLOR_BLUE := $(shell tput setaf 4)

SCRIPTS_DIR := scripts
# Cargo discovers the workspace from parent dirs, but `cargo-deny` requires a manifest path.
REPO_ROOT := $(abspath ..)

##@ Development

help: ## Display this help message
	@printf "%bAvailable targets:%b\n" "$(COLOR_BOLD)" "$(COLOR_RESET)"
	@printf "\n"
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | \
		awk 'BEGIN {FS = ":.*?## "; green="$(COLOR_GREEN)"; reset="$(COLOR_RESET)"}; {printf "  %s%-20s%s %s\n", green, $$1, reset, $$2}'
	@printf "\n"

fmt: ## Format all Rust code
	@printf "%bFormatting code...%b\n" "$(COLOR_BLUE)" "$(COLOR_RESET)"
	cargo fmt --all

clippy: ## Run clippy on all targets and features (-D warnings)
	@printf "%bRunning clippy...%b\n" "$(COLOR_BLUE)" "$(COLOR_RESET)"
	cargo clippy --all-targets --all-features -- -D warnings

check: ## Check compilation without a full debug build
	@printf "%bChecking compilation...%b\n" "$(COLOR_BLUE)" "$(COLOR_RESET)"
	cargo check

# This crate has no library target — test the binary crate explicitly.
test: ## Run tests (single-threaded)
	@printf "%bRunning tests...%b\n" "$(COLOR_BLUE)" "$(COLOR_RESET)"
	cargo test --bin aur-pkgbuilder -- --test-threads=1

test-ignored: ## Run ignored tests
	@printf "%bRunning ignored tests...%b\n" "$(COLOR_BLUE)" "$(COLOR_RESET)"
	cargo test --bin aur-pkgbuilder -- --ignored --test-threads=1

build: ## Build debug binary
	@printf "%bBuilding debug...%b\n" "$(COLOR_BLUE)" "$(COLOR_RESET)"
	cargo build

release: ## Build release binary
	@printf "%bBuilding release...%b\n" "$(COLOR_BLUE)" "$(COLOR_RESET)"
	cargo build --release

install: ## Install with cargo from the repository root
	@printf "%bInstalling binary...%b\n" "$(COLOR_BLUE)" "$(COLOR_RESET)"
	cargo install --locked --path ..

##@ Quality / policy

pre-commit: fmt clippy check test deny ## fmt, clippy, check, test, cargo-deny
	@printf "%b✓ All pre-commit checks passed!%b\n" "$(COLOR_GREEN)" "$(COLOR_RESET)"

all: pre-commit ## Alias for pre-commit

deny: ## Run cargo-deny (licenses, advisories, bans)
	@printf "%bRunning cargo-deny...%b\n" "$(COLOR_BLUE)" "$(COLOR_RESET)"
	cd "$(REPO_ROOT)" && cargo deny check

gitleaks: ## Scan repo for secrets (requires gitleaks on PATH)
	@printf "%bRunning gitleaks...%b\n" "$(COLOR_BLUE)" "$(COLOR_RESET)"
	gitleaks detect --source .. --verbose

##@ Analysis / docs (scripts in dev/scripts)

cognitive: ## Clippy cognitive-complexity report
	@printf "%bCognitive complexity report...%b\n" "$(COLOR_BLUE)" "$(COLOR_RESET)"
	@bash $(SCRIPTS_DIR)/clippy_cognitive_complexity.sh

errors: ## Summarize clippy errors
	@printf "%bClippy errors summary...%b\n" "$(COLOR_BLUE)" "$(COLOR_RESET)"
	@bash $(SCRIPTS_DIR)/clippy_errors.sh

docs: ## Generate rustdoc (no-deps) + shallow cargo tree
	@printf "%bGenerating docs...%b\n" "$(COLOR_BLUE)" "$(COLOR_RESET)"
	@bash $(SCRIPTS_DIR)/generate_docs.sh

##@ Cleanup

clean: ## cargo clean
	@printf "%bCleaning...%b\n" "$(COLOR_BLUE)" "$(COLOR_RESET)"
	cargo clean

clean-all: clean ## Remove target and script artifacts
	@printf "%bCleaning generated files...%b\n" "$(COLOR_BLUE)" "$(COLOR_RESET)"
	@rm -rf ../target/
	@rm -f $(SCRIPTS_DIR)/*.txt
	@rm -rf $(SCRIPTS_DIR)/Modules/
