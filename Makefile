export TARGET := riscv64imac-unknown-none-elf
export MODE   ?= debug

export ROOT			:= $(shell pwd)
export TARGET_DIR 	:= $(ROOT)/target/$(TARGET)/$(MODE)
export DEBUG_DIR   	:= $(ROOT)/debug

.PHONY: all build run debug test clean

all: build

build:
	cd mizu/kernel && make build

run: build
	qemu-system-riscv64 \
		-monitor stdio \
		-machine virt \
		-bios default \
		-kernel kernel-qemu \
		-nographic \
		-serial file:debug/qemu.log \
		-smp 4 -m 2G

debug: build
	qemu-system-riscv64 \
		-monitor stdio \
		-machine virt \
		-bios default \
		-kernel kernel-qemu \
		-nographic \
		-serial file:debug/qemu.log \
		-smp 4 -m 2G -s -S

test:
	cd mizu/kernel && make test

clean:
	cargo clean