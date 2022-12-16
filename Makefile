MAKEFILE_DIR := $(dir $(lastword $(MAKEFILE_LIST)))
include $(MAKEFILE_DIR)/standard_defs.mk

export OPENSSL_STATIC=1
export DOCKER_BUILDKIT=1
export COMPOSE_DOCKER_CLI_BUILD=1


IMAGES := chronicle chronicle-tp chronicle-builder
ARCHS := amd64 arm64
HOST_ARCHITECTURE ?= $(shell uname -m | sed -e 's/x86_64/amd64/' -e 's/aarch64/arm64/')

CLEAN_DIRS := $(CLEAN_DIRS)

clean: clean_containers clean_target

distclean: clean_docker clean_markers

analyze: analyze_fossa

publish: gh-create-draft-release
	mkdir -p target/arm64
	mkdir -p target/amd64
	container_id=$$(docker create chronicle-tp-amd64:${ISOLATION_ID}); \
		docker cp $$container_id:/usr/local/bin/chronicle_sawtooth_tp `pwd`/target/amd64/;  \
		docker rm $$container_id;
	container_id=$$(docker create chronicle-amd64:${ISOLATION_ID}); \
		docker cp $$container_id:/usr/local/bin/chronicle `pwd`/target/amd64/; \
		docker rm $$container_id;
	container_id=$$(docker create chronicle-tp-arm64:${ISOLATION_ID}); \
		docker cp $$container_id:/usr/local/bin/chronicle_sawtooth_tp `pwd`/target/arm64;  \
		docker rm $$container_id;
	container_id=$$(docker create chronicle-arm64:${ISOLATION_ID}); \
		docker cp $$container_id:/usr/local/bin/chronicle `pwd`/target/arm64; \
		docker rm $$container_id;
	if [ "$(RELEASABLE)" = "yes" ]; then \
		$(GH_RELEASE) upload $(VERSION) target/* ; \
	fi

PHONY: build-end-to-end-test
build-end-to-end-test:
	docker build -t chronicle-test:$(ISOLATION_ID) -f docker/chronicle-test/chronicle-test.dockerfile .

.PHONY: test-e2e
.ONESHELL:
SHELL = /bin/bash
.SHELLOPTS = $(if $(SHELLOPTS),$(SHELLOPTS):)pipefail:errexit
test-e2e: build-end-to-end-test
	docker-compose -f docker/chronicle.yaml up --force-recreate --detach
	function stopStack {
		docker logs docker-chronicle-sawtooth-tp-1
		docker logs docker-chronicle-sawtooth-api-1
		docker logs docker-validator-1
		docker-compose -f docker/chronicle.yaml down || true
	}
	trap stopStack EXIT
	docker run --network docker_default chronicle-test:$(ISOLATION_ID)

run:
	docker-compose -f docker/chronicle.yaml up --force-recreate

.PHONY: stop
stop:
	docker-compose -f docker/chronicle.yaml down || true

$(MARKERS)/binfmt:
	mkdir -p $(MARKERS)
	if [ `uname -m` = "x86_64" ]; then \
		docker run --rm --privileged multiarch/qemu-user-static --reset -p yes; \
	fi
	touch $@

# Run the compiler for host and target, then extract the binaries
.PHONY: tested-$(ISOLATION_ID)
tested-$(ISOLATION_ID): ensure-context-chronicle
	docker buildx build $(DOCKER_PROGRESS)  \
		-f./docker/unified-builder \
		-t tested-artifacts:$(ISOLATION_ID) . \
		--builder ctx-$(ISOLATION_ID) \
		--platform linux/$(HOST_ARCHITECTURE) \
		--target tested-artifacts \
	  --cache-to type=local,dest=.buildx-cache \
    --cache-from type=local,src=.buildx-cache \
		--load

	rm -rf .artifacts
	mkdir -p .artifacts

	container_id=$$(docker create tested-artifacts:${ISOLATION_ID}); \
		docker cp $$container_id:/artifacts `pwd`/.artifacts/;  \
		docker rm $$container_id;

.PHONY: test
test: tested-$(ISOLATION_ID)

define multi-arch-docker =

.PHONY: ensure-context-$(1)
$(1)-$(2)-ensure-context: $(MARKERS)/binfmt
	docker buildx create --name ctx-$(ISOLATION_ID) \
		--driver docker-container \
		--bootstrap || true
	docker buildx use ctx-$(ISOLATION_ID)

.PHONY: $(1)-$(2)-build
$(1)-$(2)-build: $(1)-$(2)-ensure-context tested-$(ISOLATION_ID)
	docker buildx build $(DOCKER_PROGRESS)  \
		-f./docker/unified-builder \
		-t $(1)-$(2):$(ISOLATION_ID) . \
		--builder ctx-$(ISOLATION_ID) \
		--build-arg TARGETARCH=$(2) \
		--platform linux/$(2) \
		--target $(1) \
		--load

$(1)-manifest: $(1)-$(2)-build
	docker manifest create $(1):$(ISOLATION_ID) \
		-a $(1)-$(2):$(ISOLATION_ID)

$(1): $(1)-$(2)-build

build: $(1)

build-native: $(1)-$(HOST_ARCHITECTURE)-build
endef

$(foreach image,$(IMAGES),$(foreach arch,$(ARCHS),$(eval $(call multi-arch-docker,$(image),$(arch)))))

clean_containers:
	docker-compose -f docker/chronicle.yaml rm -f || true

clean_docker: stop
	docker-compose -f docker/chronicle.yaml down -v --rmi all || true

clean_target:
	$(RM) -r target
