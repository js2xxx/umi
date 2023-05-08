KERNEL := mizu

KERNEL_RAW 	:= $(TARGET_DIR)/$(KERNEL)
KERNEL_SYM 	:= $(DEBUG_DIR)/$(KERNEL).sym
KERNEL_META := $(DEBUG_DIR)/$(KERNEL).txt
KERNEL_ASM  := $(DEBUG_DIR)/$(KERNEL).asm

CARGO_ARGS := --target $(TARGET) \
	--no-default-features \
	--features $(BOARD) \
	-Zbuild-std=core,compiler_builtins,alloc,panic_abort \
	-Zbuild-std-features=compiler-builtins-mem

ifeq ($(MODE),release)
    CARGO_ARGS += --release
endif

.PHONY: build

build:
	mkdir -p .cargo
	cp -rf cargo-config/* .cargo
	cargo build $(CARGO_ARGS)
	cp $(KERNEL_RAW)  $(KERNEL_SYM)
	rust-strip --strip-debug --strip-symbol="" \
		$(KERNEL_RAW) -o $(ROOT)/kernel-qemu
	rust-objdump $(ROOT)/kernel-qemu -CxR > $(KERNEL_META)
	rust-objdump $(ROOT)/kernel-qemu -Cd > $(KERNEL_ASM)
