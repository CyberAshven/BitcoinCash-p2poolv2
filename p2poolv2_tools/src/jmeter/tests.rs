// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;

#[test]
fn leading_number_reads_a_prefix() {
    assert_eq!(leading_number("123"), 123.0);
    assert_eq!(leading_number(" 12.5ms"), 12.5);
    assert_eq!(leading_number("abc"), 0.0);
}

#[test]
fn summarize_jtl_reports_per_sampler_percentiles() {
    let directory = tempfile::tempdir().expect("temp dir");
    let path = directory.path().join("run.jtl");
    std::fs::write(
        &path,
        concat!(
            "timeStamp,elapsed,label,responseCode,responseMessage,threadName,dataType,success\n",
            "1,30,submit,200,OK,t1,text,true\n",
            "2,10,submit,200,OK,t1,text,true\n",
            "3,20,submit,500,ERR,t1,text,false\n",
            "4,5,subscribe,200,OK,t1,text,true\n",
        ),
    )
    .expect("write jtl");
    assert_eq!(
        summarize_jtl(&path, "Default build (portable)"),
        concat!(
            "\n",
            "=== Default build (portable) ===\n",
            "  Total samples: 4  Errors: 1\n",
            "\n",
            "  submit               samples=3        errors=1      avg=20.0     p50=20       p95=30       p99=30       max=30\n",
            "  subscribe            samples=1        errors=0      avg=5.0      p50=5        p95=5        p99=5        max=5\n",
        )
    );
}

#[test]
fn summarize_jtl_of_a_missing_file() {
    assert_eq!(
        summarize_jtl(Path::new("/nonexistent/run.jtl"), "x"),
        "  No results file found: /nonexistent/run.jtl\n"
    );
}

#[test]
fn summarize_jtl_with_only_a_header() {
    let directory = tempfile::tempdir().expect("temp dir");
    let path = directory.path().join("run.jtl");
    std::fs::write(&path, "timeStamp,elapsed,label\n").expect("write jtl");
    assert!(summarize_jtl(&path, "x").starts_with("  No samples in "));
}
