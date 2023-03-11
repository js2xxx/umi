KERNEL := mizu

KERNEL_RAW 	:= $(TARGET_DIR)/$(KERNEL)
KERNEL_SYM 	:= $(DEBUG_DIR)/$(KERNEL).sym
KERNEL_META := $(DEBUG_DIR)/$(KERNEL).txt
KERNEL_ASM  := $(DEBUG_DIR)/$(KERNEL).asm

CARGO_ARGS := --target $(TARGET) \
	--no-default-features \
	--features $(BOARD)

ifeq ($(MODE),release)
    CARGO_ARGS += --release
endif

.PHONY: build test

build:
	cargo build $(CARGO_ARGS)
	cp $(KERNEL_RAW)  $(KERNEL_SYM)
	llvm-strip --strip-debug --strip-symbol="" \
		$(KERNEL_RAW) -o $(ROOT)/kernel-qemu
	llvm-readelf $(ROOT)/kernel-qemu -Ces > $(KERNEL_META)
	llvm-objdump $(ROOT)/kernel-qemu -Cd > $(KERNEL_ASM)

test:
ifeq ($(MODE),debug)
	cargo test --workspace
else
	cargo bench --workspace
endif
