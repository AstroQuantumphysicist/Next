/* Original Linux arch_prctl fixture; separate process images own TLS data. */
.intel_syntax noprefix
.global _start
.section .text
_start:
    mov eax, 158
    mov edi, 0x1003
    lea rsi, [rip + result]
    syscall
    test rax, rax
    jne failed
    cmp qword ptr [rip + result], 0
    jne failed
    mov eax, 158
    mov edi, 0x1004
    lea rsi, [rip + result]
    syscall
    test rax, rax
    jne failed
    cmp qword ptr [rip + result], 0
    jne failed
    mov eax, 39
    syscall
    mov r14, rax
    mov qword ptr [rip + tls_fs], r14
    not rax
    mov qword ptr [rip + tls_gs], rax
    lea r12, [rip + tls_fs]
    lea r13, [rip + tls_gs]
    mov eax, 158
    mov edi, 0x1002
    mov rsi, r12
    syscall
    test rax, rax
    jne failed
    mov eax, 158
    mov edi, 0x1001
    mov rsi, r13
    syscall
    test rax, rax
    jne failed
    mov eax, 158
    mov edi, 0x1003
    lea rsi, [rip + result]
    syscall
    cmp qword ptr [rip + result], r12
    jne failed
    mov eax, 158
    mov edi, 0x1004
    lea rsi, [rip + result]
    syscall
    cmp qword ptr [rip + result], r13
    jne failed
    mov eax, 158
    mov edi, 0xffff
    xor esi, esi
    syscall
    cmp rax, -22
    jne failed
    mov eax, 158
    mov edi, 0x1003
    mov rsi, 0x4000040000
    syscall
    cmp rax, -14
    jne failed
.ifdef NEXT_CROSS
    mov eax, 158
    mov edi, 0x1002
    mov rsi, 0x4000040000
    syscall
    cmp rax, -22
    jne failed
    mov eax, 158
    mov edi, 0x1002
    mov rsi, 0xffff800000000000
    syscall
    cmp rax, -22
    jne failed
.endif
    mov eax, 228
    mov edi, 1
    lea rsi, [rip + timestamp]
    syscall
    test rax, rax
    jne failed
    mov r15, qword ptr [rip + timestamp]
    imul r15, r15, 1000000000
    add r15, qword ptr [rip + timestamp + 8]
.ifdef NEXT_CROSS
    add r15, 2000000000
.else
    add r15, 200000000
.endif
loop:
.ifdef NEXT_CROSS
    /* NT is user-settable, but IRETQ cannot safely return it in long mode. */
    pushfq
    or qword ptr [rsp], 0x4000
    popfq
.endif
    mov r10d, 8000000
tls_spin:
    cmp qword ptr fs:[0], r14
    jne failed
    sub r10, 1
    jnz tls_spin
    cmp qword ptr fs:[0], r14
    jne failed
    mov rax, r14
    not rax
    cmp qword ptr gs:[0], rax
    jne failed
    mov eax, 24
    syscall
    cmp qword ptr fs:[0], r14
    jne failed
    mov eax, 228
    mov edi, 1
    lea rsi, [rip + timestamp]
    syscall
    test rax, rax
    jne failed
    mov rax, qword ptr [rip + timestamp]
    imul rax, rax, 1000000000
    add rax, qword ptr [rip + timestamp + 8]
    cmp rax, r15
    jb loop
    /* Clear both bases, then leave a marker for an independently booted run. */
    mov eax, 158
    mov edi, 0x1002
    xor esi, esi
    syscall
    test rax, rax
    jne failed
    mov eax, 158
    mov edi, 0x1001
    xor esi, esi
    syscall
    test rax, rax
    jne failed
    mov eax, 1
    mov edi, 1
    lea rsi, [rip + marker]
    mov edx, marker_end-marker
    syscall
    cmp rax, marker_end-marker
    jne failed
    mov eax, 60
    mov edi, 47
    syscall
failed:
    mov eax, 60
    mov edi, 99
    syscall
.section .rodata
marker: .ascii "LINUX TLS ELF: FS/GS ownership/yield/bounds compatibility PASS\n"
marker_end:
.section .data
tls_fs: .quad 0
tls_gs: .quad 0
result: .quad 0
timestamp: .quad 0, 0
.section .note.GNU-stack,"",@progbits
