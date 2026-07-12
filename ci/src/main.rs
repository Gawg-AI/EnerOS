//! EnerOS Quality Gate CLI (v0.2.0)
//!
//! Runs the four quality checks (fmt, clippy, audit, test) and prints a
//! report. Exits with code 0 on PASS, 1 on FAIL.

mod error;
mod gate;

use gate::{DefaultGate, QualityGate};

fn main() -> std::process::ExitCode {
    println!("EnerOS Quality Gate v0.2.0");
    println!("==========================");

    let gate = DefaultGate::new();
    let report = gate.run_all();

    for r in &report.results {
        let mark = if r.passed { "✓" } else { "✗" };
        match &r.message {
            Some(msg) => println!("[{}] {:<8} {}ms  {}", mark, r.name, r.duration_ms, msg),
            None => println!("[{}] {:<8} {}ms", mark, r.name, r.duration_ms),
        }
    }

    println!("==========================");
    if report.overall_pass {
        println!("Overall: PASS");
        std::process::ExitCode::SUCCESS
    } else {
        println!("Overall: FAIL");
        std::process::ExitCode::FAILURE
    }
}
