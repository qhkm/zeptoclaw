.PHONY: build build-release install clean test lint fmt check all deploy deploy-zeptoclaw deploy-r8r setup

BINARY_NAME := zeptoclaw
BUILD_DIR := target
INSTALL_DIR := $(HOME)/.local/bin

# Default target
all: lint test build

# Development build
build:
	cargo build

# Optimized release build
build-release:
	cargo build --release

# Install binary to ~/.local/bin
install: build-release
	mkdir -p $(INSTALL_DIR)
	cp $(BUILD_DIR)/release/$(BINARY_NAME) $(INSTALL_DIR)/
	@echo "Installed $(BINARY_NAME) to $(INSTALL_DIR)"

# Uninstall binary
uninstall:
	rm -f $(INSTALL_DIR)/$(BINARY_NAME)
	@echo "Removed $(BINARY_NAME) from $(INSTALL_DIR)"

# Install required dev tools (idempotent)
setup:
	@which cargo-nextest > /dev/null 2>&1 && echo "cargo-nextest already installed" || \
		(echo "Installing cargo-nextest..." && cargo install cargo-nextest --locked)

# Run all tests (nextest + doc tests for CI parity)
test: setup
	cargo nextest run
	cargo test --doc

# Run tests with output
test-verbose: setup
	cargo nextest run --no-capture
	cargo test --doc -- --nocapture

# Lint with clippy
lint:
	cargo clippy -- -D warnings

# Format code
fmt:
	cargo fmt

# Check formatting without modifying
fmt-check:
	cargo fmt -- --check

# Full check (format, lint, test)
check: fmt-check lint test

# Clean build artifacts
clean:
	cargo clean

# Show release binary size
size: build-release
	@ls -lh $(BUILD_DIR)/release/$(BINARY_NAME)
	@echo ""
	@file $(BUILD_DIR)/release/$(BINARY_NAME)

# Run the agent with a test message
run:
	cargo run -- agent -m "Hello, what can you do?"

# Run in interactive mode
run-interactive:
	cargo run -- agent

# Run gateway
run-gateway:
	cargo run -- gateway

# Deploy zeptoclaw landing + docs to Cloudflare Pages
deploy-zeptoclaw:
	bash landing/deploy.sh zeptoclaw

# Deploy r8r landing to Cloudflare Pages
deploy-r8r:
	bash landing/deploy.sh r8r

# Deploy all landing pages
deploy:
	bash landing/deploy.sh all

# Show help
help:
	@echo "ZeptoClaw Rust - Build Commands"
	@echo ""
	@echo "Development:"
	@echo "  make build          - Debug build"
	@echo "  make build-release  - Optimized release build"
	@echo "  make run            - Run agent with test message"
	@echo "  make run-interactive- Run interactive agent"
	@echo ""
	@echo "Setup:"
	@echo "  make setup          - Install required dev tools (cargo-nextest)"
	@echo ""
	@echo "Quality:"
	@echo "  make test           - Run all tests (installs nextest if needed)"
	@echo "  make lint           - Run clippy linter"
	@echo "  make fmt            - Format code"
	@echo "  make check          - Full quality check"
	@echo ""
	@echo "Installation:"
	@echo "  make install        - Install to ~/.local/bin"
	@echo "  make uninstall      - Remove from ~/.local/bin"
	@echo ""
	@echo "Deploy:"
	@echo "  make deploy-zeptoclaw - Deploy zeptoclaw landing + docs"
	@echo "  make deploy-r8r       - Deploy r8r landing"
	@echo "  make deploy           - Deploy all landing pages"
	@echo ""
	@echo "Utilities:"
	@echo "  make clean          - Remove build artifacts"
	@echo "  make size           - Show release binary size"
	@echo "  make help           - Show this help"
