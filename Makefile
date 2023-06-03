export TARGET := riscv64imac-unknown-none-elf
export MODE   ?= release
export BOARD  ?= qemu-virt

export ROOT			:= $(shell pwd)
export TARGET_DIR 	:= $(ROOT)/target/$(TARGET)/$(MODE)
export DEBUG_DIR   	:= $(ROOT)/debug

export ROOTFS  ?= $(ROOT)/third-party/img/sdcard-comp3.img
export SBI ?= $(ROOT)/third-party/bin/opensbi-$(BOARD)

.PHONY: all build run debug test clean

all: build

build:
	mkdir -p .cargo
	cp -rf cargo-config/* .cargo
	mkdir -p debug
	cd mizu/kernel && make build
	cp $(SBI) $(ROOT)/sbi-qemu

QEMU_ARGS := -monitor stdio \
	-kernel kernel-qemu \
	-nographic \
	-serial file:debug/qemu.log \
	-smp 4 -m 4G \
	-drive file=$(ROOTFS),if=none,format=raw,id=x0 \
	-device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0

ifeq ($(BOARD), qemu-virt)
	QEMU_ARGS += -machine virt \
		-bios $(SBI)
endif

run: build
	qemu-system-riscv64 $(QEMU_ARGS)

debug: build
	qemu-system-riscv64 $(QEMU_ARGS) -s -S

test:
ifeq ($(MODE),debug)
	cargo test --all-targets --features test
else
	cargo bench --all-targets --features test
endif

clean:
	cargo clean