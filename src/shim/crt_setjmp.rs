//! msvcrt: `setjmp` / `longjmp` using the Windows `_JUMP_BUFFER` layout (256 bytes).

use std::arch::naked_asm;

#[unsafe(naked)]
pub unsafe extern "win64" fn setjmp_impl() {
    naked_asm!(
        "mov [rcx], rdx",
        "mov [rcx+8], rbx",
        "lea rax, [rsp+8]",
        "mov [rcx+16], rax",
        "mov [rcx+24], rbp",
        "mov [rcx+32], rsi",
        "mov [rcx+40], rdi",
        "mov [rcx+48], r12",
        "mov [rcx+56], r13",
        "mov [rcx+64], r14",
        "mov [rcx+72], r15",
        "mov rax, [rsp]",
        "mov [rcx+80], rax",
        "stmxcsr [rcx+88]",
        "fnstcw [rcx+92]",
        "movups [rcx+96], xmm6",
        "movups [rcx+112], xmm7",
        "movups [rcx+128], xmm8",
        "movups [rcx+144], xmm9",
        "movups [rcx+160], xmm10",
        "movups [rcx+176], xmm11",
        "movups [rcx+192], xmm12",
        "movups [rcx+208], xmm13",
        "movups [rcx+224], xmm14",
        "movups [rcx+240], xmm15",
        "xor eax, eax",
        "ret",
    );
}

#[unsafe(naked)]
pub unsafe extern "win64" fn longjmp_impl() {
    naked_asm!(
        "mov eax, edx",
        "test eax, eax",
        "jnz 2f",
        "mov eax, 1",
        "2:",
        "mov rbx, [rcx+8]",
        "mov rbp, [rcx+24]",
        "mov rsi, [rcx+32]",
        "mov rdi, [rcx+40]",
        "mov r12, [rcx+48]",
        "mov r13, [rcx+56]",
        "mov r14, [rcx+64]",
        "mov r15, [rcx+72]",
        "ldmxcsr [rcx+88]",
        "fldcw [rcx+92]",
        "movups xmm6, [rcx+96]",
        "movups xmm7, [rcx+112]",
        "movups xmm8, [rcx+128]",
        "movups xmm9, [rcx+144]",
        "movups xmm10, [rcx+160]",
        "movups xmm11, [rcx+176]",
        "movups xmm12, [rcx+192]",
        "movups xmm13, [rcx+208]",
        "movups xmm14, [rcx+224]",
        "movups xmm15, [rcx+240]",
        "mov rdx, [rcx+80]",
        "mov rsp, [rcx+16]",
        "jmp rdx",
    );
}

crate::shim_extra!("crt";
    "_setjmp" => setjmp_impl,
    "_setjmpex" => setjmp_impl,
    "setjmp" => setjmp_impl,
    "__intrinsic_setjmp" => setjmp_impl,
    "__intrinsic_setjmpex" => setjmp_impl,
    "longjmp" => longjmp_impl,
);
