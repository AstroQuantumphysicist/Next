/* Original Linux x86-64 file ABI fixture. argv[1] is the host scratch path. */
.intel_syntax noprefix
.global _start
.equ PAYLOAD_LENGTH, payload_end-payload
.section .text
_start:
    lea r15, [rip + default_path]
    cmp qword ptr [rsp], 2
    jb path_ready
    mov r15, qword ptr [rsp + 16]
path_ready:
    mov eax, 257
    mov rdi, -100
    mov rsi, r15
    mov edx, 0x80242 /* O_RDWR|O_CREAT|O_TRUNC|O_CLOEXEC */
    mov r10d, 0600
    syscall
    test rax, rax
    js failed
    mov r12, rax
    mov eax, 1
    mov rdi, r12
    lea rsi, [rip + payload]
    mov edx, PAYLOAD_LENGTH
    syscall
    cmp rax, PAYLOAD_LENGTH
    jne failed
    mov eax, 5
    mov rdi, r12
    lea rsi, [rip + status]
    syscall
    test rax, rax
    jne failed
    cmp qword ptr [rip + status + 48], PAYLOAD_LENGTH
    jne failed
    mov r14, qword ptr [rip + status + 8]
    mov eax, 8
    mov rdi, r12
    xor esi, esi
    xor edx, edx
    syscall
    test rax, rax
    jne failed
    mov eax, 0
    mov rdi, r12
    mov rsi, 0x4000040000
    mov edx, 8
    syscall
    cmp rax, -14
    jne failed
    mov eax, 8
    mov rdi, r12
    xor esi, esi
    mov edx, 1
    syscall
    test rax, rax
    jne failed
    mov eax, 32 /* dup, not socket */
    mov rdi, r12
    syscall
    test rax, rax
    js failed
    mov r13, rax
    mov eax, 0
    mov rdi, r12
    lea rsi, [rip + buffer]
    mov edx, 5
    syscall
    cmp rax, 5
    jne failed
    mov eax, 8
    mov rdi, r13
    xor esi, esi
    mov edx, 1
    syscall
    cmp rax, 5
    jne failed
    mov eax, 0
    mov rdi, r13
    lea rsi, [rip + buffer + 5]
    mov edx, PAYLOAD_LENGTH-5
    syscall
    cmp rax, PAYLOAD_LENGTH-5
    jne failed
    lea rsi, [rip + payload]
    lea rdi, [rip + buffer]
    mov ecx, PAYLOAD_LENGTH
    repe cmpsb
    jne failed
    mov eax, 3
    mov rdi, r12
    syscall
    test rax, rax
    jne failed
    mov eax, 3
    mov rdi, r12
    syscall
    cmp rax, -9
    jne failed
    mov eax, 5
    mov rdi, r13
    lea rsi, [rip + status]
    syscall
    test rax, rax
    jne failed
    cmp qword ptr [rip + status + 8], r14
    jne failed
    mov eax, 3
    mov rdi, r13
    syscall
    test rax, rax
    jne failed
    mov eax, 257
    mov rdi, -100
    mov rsi, r15
    mov edx, 0xc2 /* O_RDWR|O_CREAT|O_EXCL */
    mov r10d, 0600
    syscall
    cmp rax, -17
    jne failed
    mov eax, 2
    mov rdi, r15
    mov esi, 0x80401 /* O_WRONLY|O_APPEND|O_CLOEXEC */
    xor edx, edx
    syscall
    test rax, rax
    js failed
    mov r12, rax
    mov eax, 8
    mov rdi, r12
    xor esi, esi
    xor edx, edx
    syscall
    test rax, rax
    jne failed
    mov eax, 1
    mov rdi, r12
    lea rsi, [rip + suffix]
    mov edx, 1
    syscall
    cmp rax, 1
    jne failed
    mov eax, 3
    mov rdi, r12
    syscall
    test rax, rax
    jne failed
    mov eax, 2
    mov rdi, r15
    mov esi, 0x80000 /* O_RDONLY|O_CLOEXEC */
    xor edx, edx
    syscall
    test rax, rax
    js failed
    mov r12, rax
    mov eax, 1
    mov rdi, r12
    lea rsi, [rip + suffix]
    mov edx, 1
    syscall
    cmp rax, -9
    jne failed
    mov eax, 0
    mov rdi, r12
    lea rsi, [rip + buffer]
    mov edx, PAYLOAD_LENGTH+1
    syscall
    cmp rax, PAYLOAD_LENGTH+1
    jne failed
    cmp byte ptr [rip + buffer + PAYLOAD_LENGTH], '!'
    jne failed
    mov eax, 3
    mov rdi, r12
    syscall
    test rax, rax
    jne failed
    .ifdef NEXT_CROSS
    mov eax, 2
    lea rdi, [rip + forbidden_path]
    xor esi, esi
    xor edx, edx
    syscall
    cmp rax, -13
    jne failed
    .endif
    mov eax, 1
    mov edi, 1
    lea rsi, [rip + message]
    mov edx, message_end-message
    syscall
    cmp rax, message_end-message
    jne failed
    mov eax, 60
    mov edi, 46
    syscall
    ud2
failed:
    mov eax, 231
    mov edi, 99
    syscall
    ud2
.section .rodata
payload: .ascii "linux-static-file-data"
payload_end:
suffix: .ascii "!"
default_path: .asciz "/native/linux"
forbidden_path: .asciz "/etc/passwd"
message: .ascii "LINUX FILE ELF: open/read/write/seek/stat/dup/close compatibility PASS\n"
message_end:
.section .bss
.align 8
status: .skip 144
buffer: .skip 64
.section .note.GNU-stack,"",@progbits
