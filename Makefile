export TARGET := riscv64imac-unknown-none-elf
export MODE   ?= debug

export TARGET_DIR 	:= target/$(TARGET)/$(MODE)
export DEBUG_DIR   	:= debug

.PHONY: all build run debug test clean

all: build

build:
	cd mizu && make build

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
	cd mizu && make test

clean:
	cargo clean