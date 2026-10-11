/* Original Linux static anonymous-memory and brk regression fixture. */
.intel_syntax noprefix
.global _start
.section .text
_start:
    mov eax, 12
    xor edi, edi
    syscall
    test rax, rax
    jle failed
    mov r14, rax
    lea rdi, [r14 + 4096]
    mov eax, 12
    syscall
    lea rcx, [r14 + 4096]
    cmp rax, rcx
    jne failed
    cmp qword ptr [r14], 0
    jne failed
    mov qword ptr [r14], 123
    lea rdi, [r14]
    mov eax, 12
    syscall
    cmp rax, r14
    jne failed
    lea rdi, [r14 + 4096]
    mov eax, 12
    syscall
    lea rcx, [r14 + 4096]
    cmp rax, rcx
    jne failed
    cmp qword ptr [r14], 0
    jne failed
    mov rdi, r14
    mov eax, 12
    syscall
    cmp rax, r14
    jne failed
    xor edi, edi
    mov esi, 8192
    mov edx, 3
    mov r10d, 0x22
    mov r8, -1
    xor r9d, r9d
    mov eax, 9
    syscall
    cmp rax, -4095
    jae failed
    mov r12, rax
    mov rdi, rax
    xor eax, eax
    mov ecx, 1024
    repe scasq
    jne failed
    mov rax, 0x13572468abcdef09
    mov qword ptr [r12 + 4092], rax
    cmp qword ptr [r12 + 4092], rax
    jne failed
    lea rdi, [r12 + 1]
    mov esi, 4096
    mov eax, 11
    syscall
    cmp rax, -22
    jne failed
    mov rdi, r12
    xor esi, esi
    mov eax, 11
    syscall
    cmp rax, -22
    jne failed
    .ifdef NEXT_CROSS
    xor edi, edi
    mov esi, 4096
    mov edx, 7
    mov r10d, 0x22
    mov r8, -1
    xor r9d, r9d
    mov eax, 9
    syscall
    cmp rax, -1
    jne failed
    .endif
    mov rdi, r12
    mov esi, 8192
    mov eax, 11
    syscall
    test rax, rax
    jne failed
    mov rdi, r12
    mov esi, 8192
    mov edx, 3
    mov r10d, 0x22
    mov r8, -1
    xor r9d, r9d
    mov eax, 9
    syscall
    cmp rax, -4095
    jae failed
    mov r12, rax
    mov rdi, rax
    xor eax, eax
    mov ecx, 1024
    repe scasq
    jne failed
    mov rdi, r12
    mov esi, 8192
    mov eax, 11
    syscall
    test rax, rax
    jne failed
    xor edi, edi
    xor esi, esi
    mov edx, 3
    mov r10d, 0x22
    mov r8, -1
    xor r9d, r9d
    mov eax, 9
    syscall
    cmp rax, -22
    jne failed
    mov eax, 1
    mov edi, 1
    lea rsi, [rip + message]
    mov edx, message_end-message
    syscall
    cmp rax, message_end-message
    jne failed
    mov eax, 60
    mov edi, 44
    syscall
    ud2
failed:
    mov eax, 231
    mov edi, 99
    syscall
    ud2
.section .rodata
message: .ascii "LINUX MEMORY ELF: brk/mmap/munmap/zeroing compatibility PASS\n"
message_end:
.section .data
.quad 0
.section .note.GNU-stack,"",@progbits
