//! Bounded, allocation-free kernel panic metadata formatting.
use core::fmt::{self, Write};

pub fn write_header(
    out: &mut impl Write,
    panic_id: u32,
    cpu: u32,
    thread: &str,
    file: &str,
    line: u32,
    column: u32,
) -> fmt::Result {
    writeln!(
        out,
        "HELIX PANIC: id={panic_id} cpu={cpu} thread={thread} source={file}:{line}:{column} backtrace=unavailable"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_exposes_stable_identity_cpu_thread_source_and_backtrace_limit() {
        let mut output = std::string::String::new();
        write_header(&mut output, 401, 0, "kernel-bsp", "kernel.rs", 12, 8).unwrap();
        assert_eq!(
            output,
            "HELIX PANIC: id=401 cpu=0 thread=kernel-bsp source=kernel.rs:12:8 backtrace=unavailable\n"
        );
    }
}
