//! Freestanding compiler memory intrinsics. Assembly prevents LLVM from
//! rewriting a Rust byte-copy loop into a recursive call to the same intrinsic.
use core::arch::global_asm;
global_asm!(
    r#"
.global memcpy
memcpy:
    mov rax, rdi
    mov rcx, rdx
    cld
    rep movsb
    ret
.global memset
memset:
    mov r8, rdi
    mov eax, esi
    mov rcx, rdx
    cld
    rep stosb
    mov rax, r8
    ret
.global memmove
memmove:
    mov rax, rdi
    test rdx, rdx
    jz 2f
    mov rcx, rdx
    cmp rdi, rsi
    jbe 1f
    lea rdi, [rdi + rdx - 1]
    lea rsi, [rsi + rdx - 1]
    std
    rep movsb
    cld
    ret
1:
    cld
    rep movsb
2:
    ret
.global bcmp
.global memcmp
bcmp:
memcmp:
    xor eax, eax
    test rdx, rdx
    jz 4f
3:
    movzx eax, byte ptr [rdi]
    movzx ecx, byte ptr [rsi]
    sub eax, ecx
    jne 4f
    inc rdi
    inc rsi
    dec rdx
    jnz 3b
4:
    ret
"#
);
