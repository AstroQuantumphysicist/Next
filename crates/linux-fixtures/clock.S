/* Original static Linux clock and scheduler conformance fixture. */
.intel_syntax noprefix
.global _start
.section .text
_start:
    /* Linux clock_getres accepts its defined clocks and a null result, reports
       a normalized nonzero timespec, and leaves output untouched on bad IDs. */
    mov eax, 229
    mov edi, 1
    lea rsi, [rip + resolution]
    syscall
    test rax, rax
    jne failed
    cmp qword ptr [rip + resolution], 0
    jne failed
    cmp qword ptr [rip + resolution + 8], 0
    je failed
    cmp qword ptr [rip + resolution + 8], 1000000000
    jae failed
    mov eax, 229
    xor edi, edi
    xor esi, esi
    syscall
    test rax, rax
    jne failed
    mov qword ptr [rip + bad_resolution], 0x13572468
    mov eax, 229
    mov edi, 99
    lea rsi, [rip + bad_resolution]
    syscall
    cmp rax, -22
    jne failed
    cmp qword ptr [rip + bad_resolution], 0x13572468
    jne failed
    mov eax, 229
    mov edi, 1
    mov rsi, 0x400010fff8
    syscall
    cmp rax, -14
    jne failed
    mov eax, 228
    mov edi, 99
    lea rsi, [rip + before]
    syscall
    cmp rax, -22
    jne failed
    mov eax, 228
    mov edi, 1
    lea rsi, [rip + request]
    syscall
    cmp rax, -14
    jne failed
    mov eax, 228
    mov edi, 1
    mov rsi, 0x400010fff8
    syscall
    cmp rax, -14
    jne failed
    mov eax, 35
    lea rdi, [rip + negative]
    xor esi, esi
    syscall
    cmp rax, -22
    jne failed
    mov eax, 35
    lea rdi, [rip + bad_nsec]
    xor esi, esi
    syscall
    cmp rax, -22
    jne failed
    mov eax, 35
    mov rdi, 0x4000040000
    xor esi, esi
    syscall
    cmp rax, -14
    jne failed
    .ifdef NEXT_CROSS
    mov eax, 35
    lea rdi, [rip + request]
    lea rsi, [rip + request]
    syscall
    cmp rax, -14
    jne failed
    .endif
    /* Realtime is an RTC UTC anchor, independent of monotonic uptime. */
    mov eax, 228
    xor edi, edi
    lea rsi, [rip + utc_before]
    syscall
    test rax, rax
    jne failed
    cmp qword ptr [rip + utc_before], 1577836800
    jl failed
    cmp qword ptr [rip + utc_before + 8], 1000000000
    jae failed
    mov r14, qword ptr [rip + utc_before]
    mov r15, qword ptr [rip + utc_before + 8]
    mov eax, 228
    mov edi, 99
    lea rsi, [rip + utc_before]
    syscall
    cmp rax, -22
    jne failed
    cmp qword ptr [rip + utc_before], r14
    jne failed
    cmp qword ptr [rip + utc_before + 8], r15
    jne failed
    mov eax, 228
    xor edi, edi
    lea rsi, [rip + request]
    syscall
    cmp rax, -14
    jne failed
    mov eax, 228
    xor edi, edi
    mov rsi, 0x400010fff8
    syscall
    cmp rax, -14
    jne failed
    mov eax, 228
    mov edi, 1
    lea rsi, [rip + before]
    syscall
    test rax, rax
    jne failed
    mov r12, qword ptr [rip + before]
    imul r12, r12, 1000000000
    add r12, qword ptr [rip + before + 8]
    cmp qword ptr [rip + before + 8], 1000000000
    jae failed
    mov r13, 0x13572468abcdef09
    movq xmm15, r13
    mov eax, 35
    lea rdi, [rip + request]
    lea rsi, [rip + remaining]
    syscall
    test rax, rax
    jne failed
    mov rax, 0x13572468abcdef09
    cmp r13, rax
    jne failed
    movq rax, xmm15
    cmp r13, rax
    jne failed
    mov eax, 228
    mov edi, 1
    lea rsi, [rip + after]
    syscall
    test rax, rax
    jne failed
    mov rax, qword ptr [rip + after]
    imul rax, rax, 1000000000
    add rax, qword ptr [rip + after + 8]
    sub rax, r12
    cmp rax, 20000000
    jl failed
    mov eax, 228
    xor edi, edi
    lea rsi, [rip + utc_after]
    syscall
    test rax, rax
    jne failed
    cmp qword ptr [rip + utc_after + 8], 1000000000
    jae failed
    mov rax, qword ptr [rip + utc_after]
    sub rax, r14
    js failed
    imul rax, rax, 1000000000
    add rax, qword ptr [rip + utc_after + 8]
    sub rax, r15
    cmp rax, 20000000
    jl failed
    mov eax, 1
    mov edi, 1
    lea rsi, [rip + message]
    mov edx, message_end-message
    syscall
    cmp rax, message_end-message
    jne failed
    mov eax, 60
    mov edi, 43
    syscall
    ud2
failed:
    mov eax, 231
    mov edi, 99
    syscall
    ud2
.section .rodata
request: .quad 0, 20000000
negative: .quad -1, 0
bad_nsec: .quad 0, 1000000000
message: .ascii "LINUX CLOCK ELF: gettime/getres/monotonic/sleep/bounds compatibility PASS\n"
message_end:
.section .bss
.align 8
before: .skip 16
after: .skip 16
remaining: .skip 16
resolution: .skip 16
bad_resolution: .skip 8
utc_before: .skip 16
utc_after: .skip 16
.section .note.GNU-stack,"",@progbits
