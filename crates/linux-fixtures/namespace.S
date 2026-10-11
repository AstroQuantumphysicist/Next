/* Original Linux namespace/open-description compatibility fixture. */
.intel_syntax noprefix
.global _start
.section .text
_start:
    lea r12, [rip + old_path]
    lea r13, [rip + new_path]
    cmp qword ptr [rsp], 3
    jb paths_ready
    mov r12, qword ptr [rsp + 16]
    mov r13, qword ptr [rsp + 24]
paths_ready:
    mov eax, 2
    mov rdi, r12
    mov esi, 0x242
    mov edx, 0600
    syscall
    test rax, rax
    js failed
    mov r14, rax
    mov eax, 1
    mov rdi, r14
    lea rsi, [rip + shared]
    mov edx, 6
    syscall
    cmp rax, 6
    jne failed
    mov eax, 8
    mov rdi, r14
    xor esi, esi
    xor edx, edx
    syscall
    test rax, rax
    jne failed
    mov eax, 2
    mov rdi, r12
    xor esi, esi
    xor edx, edx
    syscall
    test rax, rax
    js failed
    mov r15, rax
    /* Independent opens start with separate cursors on one inode. */
    mov eax, 0
    mov rdi, r14
    lea rsi, [rip + buffer]
    mov edx, 3
    syscall
    cmp rax, 3
    jne failed
    mov eax, 0
    mov rdi, r15
    lea rsi, [rip + buffer + 3]
    mov edx, 3
    syscall
    cmp rax, 3
    jne failed
    cmp word ptr [rip + buffer], 0x6873
    jne failed
    cmp word ptr [rip + buffer + 3], 0x6873
    jne failed
    mov eax, 264
    mov rdi, -100
    mov rsi, r12
    mov rdx, -100
    mov r10, r13
    syscall
    test rax, rax
    jne failed
    mov eax, 2
    mov rdi, r12
    xor esi, esi
    xor edx, edx
    syscall
    cmp rax, -2
    jne failed
    mov eax, 2
    mov rdi, r13
    xor esi, esi
    xor edx, edx
    syscall
    test rax, rax
    js failed
    mov rbp, rax
    mov eax, 5
    mov rdi, r14
    lea rsi, [rip + status]
    syscall
    test rax, rax
    jne failed
    mov rbx, qword ptr [rip + status + 8]
    mov eax, 5
    mov rdi, rbp
    lea rsi, [rip + status]
    syscall
    test rax, rax
    jne failed
    cmp qword ptr [rip + status + 8], rbx
    jne failed
    mov eax, 263
    mov rdi, -100
    mov rsi, r13
    xor edx, edx
    syscall
    test rax, rax
    jne failed
    mov eax, 2
    mov rdi, r13
    xor esi, esi
    xor edx, edx
    syscall
    cmp rax, -2
    jne failed
    /* Write detached inode and observe it from another retained description. */
    mov eax, 1
    mov rdi, r14
    lea rsi, [rip + changed]
    mov edx, 3
    syscall
    cmp rax, 3
    jne failed
    mov eax, 8
    mov rdi, r15
    xor esi, esi
    xor edx, edx
    syscall
    test rax, rax
    jne failed
    mov eax, 0
    mov rdi, r15
    lea rsi, [rip + buffer]
    mov edx, 6
    syscall
    cmp rax, 6
    jne failed
    lea rsi, [rip + detached]
    lea rdi, [rip + buffer]
    mov ecx, 6
    repe cmpsb
    jne failed
    mov eax, 2
    mov rdi, r13
    mov esi, 0x242
    mov edx, 0600
    syscall
    test rax, rax
    js failed
    mov r15, rax
    mov eax, 5
    mov rdi, r15
    lea rsi, [rip + status]
    syscall
    test rax, rax
    jne failed
    cmp qword ptr [rip + status + 8], rbx
    je failed
    mov eax, 1
    mov rdi, r15
    lea rsi, [rip + fresh]
    mov edx, 5
    syscall
    cmp rax, 5
    jne failed
    mov eax, 82
    mov rdi, r13
    mov rsi, r12
    syscall
    test rax, rax
    jne failed
    mov eax, 87
    mov rdi, r12
    syscall
    test rax, rax
    jne failed
.ifdef NEXT_CROSS
    mov eax, 87
    lea rdi, [rip + outside]
    syscall
    cmp rax, -13
    jne failed
    mov eax, 82
    lea rdi, [rip + outside]
    mov rsi, r13
    syscall
    cmp rax, -13
    jne failed
    mov eax, 87
    mov rdi, 0x4000040000
    syscall
    cmp rax, -14
    jne failed
    mov eax, 87
    lea rdi, [rip + relative]
    syscall
    cmp rax, -38
    jne failed
    mov eax, 263
    mov rdi, -100
    mov rsi, r13
    mov edx, 512
    syscall
    cmp rax, -38
    jne failed
.endif
    /* Exit also closes the earlier independent descriptor retained at fd4. */
    mov eax, 1
    mov edi, 1
    lea rsi, [rip + marker]
    mov edx, marker_end-marker
    syscall
    cmp rax, marker_end-marker
    jne failed
    mov eax, 60
    mov edi, 48
    syscall
failed:
    mov eax, 60
    mov edi, 99
    syscall
.section .rodata
old_path: .asciz "/native/ns-old"
new_path: .asciz "/native/ns-new"
outside: .asciz "/outside/ns"
relative: .asciz "relative"
shared: .ascii "shared"
changed: .ascii "RED"
detached: .ascii "shaRED"
fresh: .ascii "fresh"
marker: .ascii "LINUX NAMESPACE ELF: rename/unlink/open-description/inode compatibility PASS\n"
marker_end:
.section .data
buffer: .space 16
status: .space 144
.section .note.GNU-stack,"",@progbits
