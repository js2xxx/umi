KERNEL := mizu

KERNEL_RAW 	:= ../$(TARGET_DIR)/$(KERNEL)
KERNEL_SYM 	:= ../$(DEBUG_DIR)/$(KERNEL).sym
KERNEL_META := ../$(DEBUG_DIR)/$(KERNEL).txt

.PHONY: build

build:
ifeq ($(MODE),debug)
	cargo build --target $(TARGET)
else
	cargo build --release --target $(TARGET)
endif
	llvm-strip --only-keep-debug $(KERNEL_RAW) -o $(KERNEL_SYM)
	llvm-strip --strip-debug $(KERNEL_RAW) -o ../kernel-qemu
	llvm-objdump ../kernel-qemu -x > $(KERNEL_META)
