ENTRY(_start)
PHDRS { text PT_LOAD FLAGS(5); rodata PT_LOAD FLAGS(4); data PT_LOAD FLAGS(6); }
get_std_handle_iat = 0x4000003040;
write_file_iat = 0x4000003048;
exit_process_iat = 0x4000003050;
SECTIONS {
    . = 0x4000001000;
    .text : { *(.text*) } :text
    . = 0x4000002000;
    .rdata : { *(.rdata*) } :rodata
    . = 0x4000004000;
    .data : { *(.data*) } :data
    /DISCARD/ : { *(.note*) *(.comment) *(.eh_frame*) }
}
ASSERT(SIZEOF(.text) <= 4096, "PE fixture text exceeds one page")
ASSERT(SIZEOF(.rdata) <= 4096, "PE fixture read-only data exceeds one page")
ASSERT(SIZEOF(.data) <= 4096, "PE fixture data exceeds one page")
