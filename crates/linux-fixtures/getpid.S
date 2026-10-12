/* Original Linux x86-64 static fixture for the process-identity syscall. */
.intel_syntax noprefix
.global _start
.section .text
_start:
    cmp qword ptr [rsp], 1
    jne failed
    cmp qword ptr [rsp + 16], 0
    jne failed

    mov r12, 0x246813579abcdef0
    mov eax, 39 /* getpid */
    syscall
    test rax, rax
    jle failed
    mov r13, rax
    mov eax, 39
    syscall
    cmp rax, r13
    jne failed
    mov rax, 0x246813579abcdef0
    cmp r12, rax
    jne failed

    mov eax, 1 /* write */
    mov edi, 1
    lea rsi, [rip + message]
    mov edx, message_end-message
    syscall
    cmp rax, message_end-message
    jne failed
    mov eax, 60
    mov edi, 51
    syscall
    ud2
failed:
    mov eax, 231
    mov edi, 99
    syscall
    ud2
.section .rodata
message:
    .ascii "LINUX GETPID ELF: stable kernel process identity PASS\n"
message_end:
.section .bss
.skip 8
.section .note.GNU-stack,"",@progbits
