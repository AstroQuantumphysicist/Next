/* Independent Linux x86-64 static executable: no Next note or runtime. */
.intel_syntax noprefix
.global _start
.section .text
_start:
    cmp qword ptr [rsp], 1
    jne failed
    cmp qword ptr [rsp + 16], 0
    jne failed
    mov r12, 0x13572468abcdef09
    movq xmm15, r12
    mov eax, 39
    syscall
    test rax, rax
    jle failed
    mov eax, 999
    syscall
    cmp rax, -38
    jne failed
    mov eax, 1
    mov edi, 1
    mov rsi, 0x4000040000
    mov edx, 1
    syscall
    cmp rax, -14
    jne failed
    mov eax, 1
    mov edi, 99
    lea rsi, [rip + message]
    mov edx, message_end-message
    syscall
    cmp rax, -9
    jne failed
    .ifdef NEXT_CROSS
    /* Development gate test: Linux must not enter native SystemReady. */
    mov eax, 15
    xor edi, edi
    int 0x80
    cmp rax, -38
    jne failed
    .endif
    mov eax, 24
    syscall
    test rax, rax
    jne failed
    mov rax, 0x13572468abcdef09
    cmp r12, rax
    jne failed
    movq rax, xmm15
    cmp r12, rax
    jne failed
    mov eax, 1
    mov edi, 1
    lea rsi, [rip + message]
    mov edx, message_end-message
    syscall
    cmp rax, message_end-message
    jne failed
    mov eax, 60
    mov edi, 37
    syscall
    ud2
failed:
    mov eax, 231
    mov edi, 99
    syscall
    ud2
.section .rodata
message:
    .ascii "LINUX STATIC ELF: original syscall compatibility PASS\n"
message_end:
.section .data
    .quad 0x9876543210abcdef
.section .bss
    .skip 16
.section .note.GNU-stack,"",@progbits
