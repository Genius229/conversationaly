// CI-only executable: must fail Intel SDE's Ivy Bridge instruction checker.
#[inline(never)]
#[target_feature(enable = "avx2")]
unsafe fn execute_avx2() {
    // Integer XOR on a 256-bit YMM register requires AVX2, not merely AVX.
    unsafe { core::arch::asm!("vpxor ymm0, ymm0, ymm0", out("ymm0") _, options(nostack, nomem)); }
}

fn main() {
    unsafe { execute_avx2(); }
}
