KERNEL := mizu

KERNEL_RAW 	:= ../$(TARGET_DIR)/$(KERNEL)
KERNEL_SYM 	:= ../$(DEBUG_DIR)/$(KERNEL).sym
KERNEL_META := ../$(DEBUG_DIR)/$(KERNEL).txt
KERNEL_ASM  := ../$(DEBUG_DIR)/$(KERNEL).asm

.PHONY: build

build:
ifeq ($(MODE),debug)
	cargo build --target $(TARGET)
else
	cargo build --release --target $(TARGET)
endif
	cp $(KERNEL_RAW)  $(KERNEL_SYM)
	llvm-strip --strip-debug $(KERNEL_RAW) -o ../kernel-qemu
	llvm-objdump ../kernel-qemu -x > $(KERNEL_META)
	llvm-objdump ../kernel-qemu -Cd > $(KERNEL_ASM)
