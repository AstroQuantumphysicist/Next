// Unactivated AP-only bootstrap. Its storage is immutable kernel rodata;
// a reviewed caller must copy this whole blob to its owned SIPI page.
.section .rodata.ap_startup,"a",@progbits
.balign 16
.global next_ap_blob_start, next_ap_pm32, next_ap_lm64, next_ap_blob_end
next_ap_blob_start:
.code16
cli
cld
// Identify this AP using only owned low records before any high-memory read.
// DS=0x8200 avoids crossing real-mode segment limits for metadata at 0x82000.
mov ax, 0x8200
mov ds, ax
mov eax, 1
cpuid
shr ebx, 24
xor esi, esi
mov cx, 16
next_ap_low_find:
cmp dword ptr [si], ebx
jne next_ap_low_next
cmp word ptr [si + 12], 1
je next_ap_low_stack
next_ap_low_next:
add si, 16
loop next_ap_low_find
jmp next_ap_emergency16
next_ap_low_stack:
mov ax, word ptr [si + 8]
mov ss, ax
mov sp, word ptr [si + 10] // immediately follows MOV SS interrupt shadow
// Full private-handler IVT, selected only after this AP owns its low stack.
// Pre-first-instruction NMI and non-NMI transition faults remain blocked.
lidt [3392]
xor ax, ax
mov ds, ax
mov es, ax
lgdt cs:[next_ap_gdt_ptr_offset]
// Shared vector2 descriptors survive PE changing IDTR interpretation.
// Only NMI is covered; no pre-entry or other fault/quiescence claim.
mov ax, 0x8200
mov ds, ax
lidt [3416] // owned metadata offset fits the real-mode segment limit
xor ax, ax
mov ds, ax
mov eax, cr0
or eax, 1
mov cr0, eax
.byte 0x66, 0xea // 32-bit far offset is mandatory in .code16.
.long 0x80000 + next_ap_pm32 - next_ap_blob_start
.word 0x08
.global next_ap_emergency16, next_ap_emergency32
.code16
next_ap_emergency16:
cli
hlt
jmp next_ap_emergency16
.code32
next_ap_pm32:
mov ax, 0x10
mov ds, ax
mov es, ax
// Compute the physical private stack while retaining real-mode SS cache.
// DS is already flat; ESI is the zero-extended low record offset.
movzx eax, word ptr [0x82000 + esi + 8]
shl eax, 4
movzx edx, sp
add edx, eax
mov ax, 0x10
mov ss, ax
mov esp, edx // immediate MOV SS shadow prevents a flat-SS/old-SP window
lidt [0x82d46]
// The BSP serialized both physical witnesses from its retained allocation.
// A20-disabled high address aliases the first marker and fails before mailbox
// pointers or private high stacks are dereferenced. No unowned writes occur.
mov esi, dword ptr [0x82d20]
mov edi, dword ptr [0x82d28]
mov eax, dword ptr [esi]
cmp eax, dword ptr [0x82d30]
jne next_ap_emergency32
mov eax, dword ptr [esi + 4]
cmp eax, dword ptr [0x82d34]
jne next_ap_emergency32
mov eax, dword ptr [edi]
cmp eax, dword ptr [0x82d38]
jne next_ap_emergency32
mov eax, dword ptr [edi + 4]
cmp eax, dword ptr [0x82d3c]
jne next_ap_emergency32
mov eax, 1
cpuid
shr ebx, 24
mov edi, ebx
mov esi, offset {mailboxes}
mov ecx, 16
next_ap_find:
cmp dword ptr [esi + {state}], 1
jne next_ap_next
cmp dword ptr [esi + {expected}], edi
je next_ap_claim
next_ap_next:
add esi, {slot_bytes}
loop next_ap_find
jmp next_ap_park32
next_ap_claim:
mov eax, 1
mov edx, 2
lock cmpxchg dword ptr [esi + {state}], edx
jne next_ap_park32
// No stack is touched before its mailbox has been acquired and claimed.
mov esp, dword ptr [esi + {stack}]
mov eax, 1
cpuid
and edx, 0x06000220 // MSR, APIC, SSE, SSE2
cmp edx, 0x06000220
jne next_ap_bad32
mov eax, 0x80000000
cpuid
cmp eax, 0x80000001
jb next_ap_bad32
mov eax, 0x80000001
cpuid
and edx, 0x20100000 // Long mode and NX
cmp edx, 0x20100000
jne next_ap_bad32
mov eax, cr4
and eax, 0
or eax, 0x20 // PAE only: AP calls no SSE/Rust/runtime code.
mov cr4, eax
mov eax, dword ptr [esi + {cr3}]
mov cr3, eax
mov ecx, 0xc0000080
rdmsr
or eax, 0x900 // LME and NXE
wrmsr
mov eax, cr0
or eax, 0x80010001 // PG, WP, PE
// NMI bridge interprets vector2 correctly on both sides of LMA change.
lidt [0x82d58]
mov cr0, eax
.byte 0xea
.long 0x80000 + next_ap_lm64 - next_ap_blob_start
.word 0x18
next_ap_bad32:
mov dword ptr [esi + {failure}], 1
mov eax, 4
xchg dword ptr [esi + {state}], eax
next_ap_park32:
cli
hlt
jmp next_ap_park32
.code32
next_ap_emergency32:
cli
hlt
jmp next_ap_emergency32
.code64
next_ap_lm64:
mov ax, 0x10
mov ds, ax
mov es, ax
mov ss, ax
xor eax, eax
mov fs, ax
mov gs, ax
mov r12d, esi
mov rsp, qword ptr [r12 + {stack}]
xor ebp, ebp
// Private IDT uses this CPU's current stack; no shared TSS/IST or runtime gate.
lidt [r12 + {idtr}]
mov ecx, 0xc0000100
xor eax, eax
xor edx, edx
wrmsr
mov ecx, 0xc0000101
wrmsr
mov ecx, 0xc0000102
wrmsr
mov eax, 1
cpuid
shr ebx, 24
cmp ebx, dword ptr [r12 + {expected}]
jne next_ap_bad64
mov dword ptr [r12 + {observed}], ebx
mov rax, qword ptr [r12 + {token}]
test rax, rax
jz next_ap_bad64
mov qword ptr [r12 + {ack}], rax
mov rdx, qword ptr [r12 + {hpet}]
mov rdx, qword ptr [rdx]
cmp rdx, qword ptr [r12 + {deadline}]
jae next_ap_late64
// Ready release-CAS loses to a timeout CAS; it cannot resurrect quarantine.
mov eax, 2
mov edx, 3
lock cmpxchg dword ptr [r12 + {state}], edx
jmp next_ap_park64
next_ap_late64:
mov dword ptr [r12 + {failure}], 4
jmp next_ap_quarantine64
next_ap_bad64:
mov dword ptr [r12 + {failure}], 2
next_ap_quarantine64:
mov eax, 4
xchg dword ptr [r12 + {state}], eax
next_ap_park64:
cli
hlt
jmp next_ap_park64
.global next_ap_emergency64
next_ap_emergency64:
cli
hlt
jmp next_ap_emergency64
.balign 8
// Accessed is pre-set: CPUs must not write descriptor bytes in this RX page.
next_ap_gdt:
.quad 0
.quad 0x00cf9b000000ffff // 08 protected-mode code
.quad 0x00cf93000000ffff // 10 data
.quad 0x00af9b000000ffff // 18 long-mode code
next_ap_gdt_end:
next_ap_gdt_ptr:
.word next_ap_gdt_end - next_ap_gdt - 1
.long 0x80000 + next_ap_gdt - next_ap_blob_start
next_ap_null_idt:
.word 0
.long 0
.set next_ap_null_idt_offset, next_ap_null_idt - next_ap_blob_start
.set next_ap_gdt_ptr_offset, next_ap_gdt_ptr - next_ap_blob_start
next_ap_blob_end:
.code64
.section .text.ap_private_fault,"ax",@progbits
.global next_ap_private_fault
next_ap_private_fault:
cli
mov dword ptr [r12 + {failure}], 3
mov eax, 4
xchg dword ptr [r12 + {state}], eax
next_ap_fault_park:
hlt
jmp next_ap_fault_park
