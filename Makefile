TARGET := riscv64imac-unknown-none-elf
MODE   ?= debug

TARGET_DIR  := target/$(TARGET)/$(MODE)

KERNEL_FILE:

.PHONY: build run all

all: build

build:
ifeq ($(MODE),debug)
	cargo build --target $(TARGET)
else
	cargo build --release --target $(TARGET)
endif
	cp $(TARGET_DIR)/mizu kernel-qemu

run: build
	qemu-system-riscv64 \
		-machine virt \
		-bios default \
		-device loader,file=kernel-qemu,addr=0x80200000 \
		-kernel kernel-qemu \
		-nographic \
		-smp 4 -m 2G
