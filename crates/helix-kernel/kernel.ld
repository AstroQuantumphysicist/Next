ENTRY(helix_start)
PHDRS {
  text PT_LOAD FLAGS(5);
  rodata PT_LOAD FLAGS(4);
  data PT_LOAD FLAGS(6);
}
SECTIONS {
  . = 0x04000000;
  __kernel_start = .;
  __text_start = .;
  .text : { KEEP(*(.text.entry)) *(.text .text.*) } :text
  . = ALIGN(4096);
  __text_end = .;
  __rodata_start = .;
  .rodata : { *(.rodata .rodata.*) *(.srodata .srodata.*) } :rodata
  . = ALIGN(4096);
  __rodata_end = .;
  __data_start = .;
  .data : { *(.data .data.*) *(.sdata .sdata.*) *(.got .got.*) } :data
  .bss (NOLOAD) : { *(.bss .bss.*) *(.sbss .sbss.*) *(COMMON) } :data
  . = ALIGN(4096);
  __kernel_end = .;
  /DISCARD/ : { *(.eh_frame .eh_frame_hdr) *(.comment) *(.note .note.*) }
}
