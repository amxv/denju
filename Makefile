SHELL := /bin/bash

GO ?= go
GOFMT ?= gofmt
BIN_NAME ?= mycli
CMD_NAME ?= mycli
CMD_PATH ?= ./cmd/$(CMD_NAME)
DIST_DIR ?= dist
BIN_PATH ?= $(DIST_DIR)/$(BIN_NAME)
VERSION ?= $(shell node -p "require('./package.json').version" 2>/dev/null)
LDFLAGS ?= -s -w -X github.com/amxv/go-cli-template/internal/buildinfo.Version=$(if $(VERSION),$(VERSION),dev)

.PHONY: help bootstrap fmt test vet lint check docs-check docs-build build build-all install-local clean release-tag

help:
	@echo "go-cli-template command runner"
	@echo ""
	@echo "Targets:"
	@echo "  make bootstrap    - initialize CLI, module, repo, npm, docs, and license identity"
	@echo "  make fmt          - format Go files"
	@echo "  make test         - run go test ./..."
	@echo "  make vet          - run go vet ./..."
	@echo "  make lint         - run Node script checks"
	@echo "  make check        - fmt + test + vet + lint"
	@echo "  make docs-check    - validate the Astro/ZueDocs site"
	@echo "  make docs-build    - build the Astro/ZueDocs site"
	@echo "  make build        - build local binary to dist/mycli"
	@echo "  make build-all    - build release binaries for 6 target platforms"
	@echo "  make install-local - install CLI to ~/.local/bin/mycli"
	@echo "  make clean        - remove dist artifacts"
	@echo "  make release-tag VERSION=x.y.z - create and push a tag for GitHub Actions"

bootstrap:
	@node scripts/setup.js $(BOOTSTRAP_ARGS)

fmt:
	@$(GOFMT) -w $$(find . -type f -name '*.go' -not -path './dist/*')

test:
	@$(GO) test ./...

vet:
	@$(GO) vet ./...

lint:
	@npm run lint

check: fmt test vet lint

docs-check:
	@bun run docs:check

docs-build:
	@bun run docs:build

build:
	@mkdir -p $(DIST_DIR)
	@$(GO) build -trimpath -ldflags="$(LDFLAGS)" -o $(BIN_PATH) $(CMD_PATH)

build-all:
	@mkdir -p $(DIST_DIR)
	@for target in "darwin amd64" "darwin arm64" "linux amd64" "linux arm64" "windows amd64"; do \
		set -- $$target; \
		GOOS=$$1; GOARCH=$$2; \
		EXT=""; \
		if [ "$$GOOS" = "windows" ]; then EXT=".exe"; fi; \
		echo "Building $(BIN_NAME) for $$GOOS/$$GOARCH"; \
		CGO_ENABLED=0 GOOS=$$GOOS GOARCH=$$GOARCH $(GO) build -trimpath -ldflags="$(LDFLAGS)" -o "$(DIST_DIR)/$(BIN_NAME)_$$GOOS_$$GOARCH$$EXT" $(CMD_PATH); \
	done

install-local: build
	@mkdir -p $$HOME/.local/bin
	@install -m 755 $(BIN_PATH) $$HOME/.local/bin/$(BIN_NAME)
	@echo "Installed $(BIN_NAME) to $$HOME/.local/bin/$(BIN_NAME)"

clean:
	@rm -rf $(DIST_DIR)

release-tag:
	@test -n "$(VERSION)" || (echo "Usage: make release-tag VERSION=x.y.z" && exit 1)
	@git tag "v$(VERSION)"
	@git push origin "v$(VERSION)"
