KERNEL := mizu

KERNEL_RAW 	:= $(TARGET_DIR)/$(KERNEL)
KERNEL_SYM 	:= $(DEBUG_DIR)/$(KERNEL).sym
KERNEL_META := $(DEBUG_DIR)/$(KERNEL).txt
KERNEL_ASM  := $(DEBUG_DIR)/$(KERNEL).asm

.PHONY: build test

build:
ifeq ($(MODE),debug)
	cargo build --target $(TARGET)
else
	cargo build --release --target $(TARGET)
endif
	cp $(KERNEL_RAW)  $(KERNEL_SYM)
	llvm-strip --strip-debug $(KERNEL_RAW) -o $(ROOT)/kernel-qemu
	llvm-objdump $(ROOT)/kernel-qemu -x > $(KERNEL_META)
	llvm-objdump $(ROOT)/kernel-qemu -Cd > $(KERNEL_ASM)

test:
ifeq ($(MODE),debug)
	cargo test
else
	cargo bench
endif
