// SPDX-License-Identifier: AGPL-3.0-or-later

//! Offline direct-split cost table and exact standard-profile boundary.

use std::env::args;

use p2poolv2_photon_payout::lean::program::{compile_program, generated_source};
use p2poolv2_photon_payout::lean::test_utils::{
    FIXTURE_COST, build_fixture, claim_amount_fixture, mixed_profile, profile_fixture, verify_both,
};
use p2poolv2_photon_payout::state::RootLock;
use p2poolv2_photon_payout::transaction::output_dust;
use p2poolv2_photon_payout::vm::measure_input;

fn row(count: usize, length: usize) -> Result<(), String> {
    let split = profile_fixture(count, length)?;
    verify_both(&split)?;
    let standard = measure_input(&split.transaction, &split.source_outputs, 0, true)?;
    let nonstandard = measure_input(&split.transaction, &split.source_outputs, 0, false)?;
    println!(
        "| {count} ({length}) | {} | {} | {:.0}/{:.0}; {:.0}/{:.0} | {:.0}/{:.0}; {:.0}/{:.0} | {} | {:.3} | {} / {} / {} | {} | {} | {} | {} |",
        split.redeem.len(),
        standard.unlocking_length,
        standard.operation_cost,
        standard.operation_cost_limit,
        nonstandard.operation_cost,
        nonstandard.operation_cost_limit,
        standard.hash_iterations,
        standard.hash_iteration_limit,
        nonstandard.hash_iterations,
        nonstandard.hash_iteration_limit,
        split.size,
        split.fee as f64 / count as f64,
        output_dust(&split.transaction.outputs[0]),
        output_dust(&split.transaction.outputs[count]),
        output_dust(split.transaction.outputs.last().expect("change")),
        split.gross_fronting,
        split.external_satoshis,
        standard.maximum_stack_item,
        standard.peak_memory_slots
    );
    Ok(())
}

fn main() -> Result<(), String> {
    if args().nth(1).as_deref() == Some("--claim-capacity") {
        let mut maximum = 187usize;
        while claim_amount_fixture(maximum + 1)
            .and_then(|split| verify_both(&split))
            .is_ok()
        {
            maximum += 1;
        }
        let split = claim_amount_fixture(maximum)?;
        let standard = measure_input(&split.transaction, &split.source_outputs, 0, true)?;
        let nonstandard = measure_input(&split.transaction, &split.source_outputs, 0, false)?;
        println!(
            "Claim-amount profile maximum {maximum}:R{},unlock{},stdop{}/{},nonstdop{}/{},hash{}/{};{}/{},fee{},frontgross{},net{},peak{},item{}. Next {}",
            split.redeem.len(),
            standard.unlocking_length,
            standard.operation_cost,
            standard.operation_cost_limit,
            nonstandard.operation_cost,
            nonstandard.operation_cost_limit,
            standard.hash_iterations,
            standard.hash_iteration_limit,
            nonstandard.hash_iterations,
            nonstandard.hash_iteration_limit,
            split.fee,
            split.gross_fronting,
            split.external_satoshis,
            standard.peak_memory_slots,
            standard.maximum_stack_item,
            claim_amount_fixture(maximum + 1)
                .and_then(|split| verify_both(&split))
                .expect_err("limit")
        );
        return Ok(());
    }
    if args().nth(1).as_deref() == Some("--small-capacity")
        || args().nth(1).as_deref() == Some("--unequal-capacity")
    {
        let unequal = args().nth(1).as_deref() == Some("--unequal-capacity");
        let mut maximum = 187usize;
        let make = |count: usize| -> Result<_, String> {
            let mut manifest = mixed_profile(count)?;
            manifest.cost = 1;
            for (index, entry) in manifest.entries.iter_mut().enumerate() {
                entry.owed = if unequal {
                    (count - index) as u64 + 10
                } else {
                    20
                };
                entry.multiple = 10;
            }
            let reward = manifest.entries.iter().map(|entry| entry.owed).sum();
            build_fixture(&manifest, reward, RootLock::P2sh20)
        };
        while make(maximum + 1)
            .and_then(|split| verify_both(&split))
            .is_ok()
        {
            maximum += 1;
        }
        let split = make(maximum)?;
        let metrics = measure_input(&split.transaction, &split.source_outputs, 0, true)?;
        println!(
            "Generic covenant only (outside current PHOTON claim amount profile), C=1,multiple10,unequal={unequal}, mixed maximum{maximum}:R{},unlock{},op{}/{},fee{},front{}. Next:{}",
            split.redeem.len(),
            metrics.unlocking_length,
            metrics.operation_cost,
            metrics.operation_cost_limit,
            split.fee,
            split.external_satoshis,
            make(maximum + 1)
                .and_then(|split| verify_both(&split))
                .expect_err("limit")
        );
        return Ok(());
    }
    if args().nth(1).as_deref() == Some("--source") {
        print!("{}", generated_source());
        return Ok(());
    }
    println!("# Lean direct split measurements");
    println!();
    println!(
        "Pinned libauth be3ed16396c9eb9c0e57343474d36079a37c8dae, BCH 2026. Program {} bytes. All listed spends verify both complete VMs. Equal gross debt 20*C, C=2^32 raw tokens, multiplier20, maximum 35-byte P2SH32 payee scripts, P2PKH submitter/change, one 100-byte signed P2PKH funding unlock, 700 inherited satoshis, P2SH20 root. All payee/submitter token amounts use nine-byte CompactSize. Fee=serialized bytes at 1 sat/byte.",
        compile_program()?.len()
    );
    println!();
    println!(
        "| N | R bytes | Unlock bytes | Op cost/limit | Hash std; nonstd | Tx bytes/fee | Fee/payee | Dust payee/submitter/change | Gross front sats | Net external sats | Max item | Peak slots |"
    );
    println!("|---:|---:|---:|---:|---|---:|---:|---|---:|---:|---:|---:|");
    row(1, 35)?;
    row(8, 35)?;
    row(32, 35)?;
    row(64, 35)?;
    row(100, 35)?;
    for length in [35, 25, 23] {
        let mut maximum = 1usize;
        while profile_fixture(maximum + 1, length)
            .and_then(|split| verify_both(&split))
            .is_ok()
        {
            maximum += 1;
        }
        row(maximum, length)?;
        let next = profile_fixture(maximum + 1, length).and_then(|split| verify_both(&split));
        println!(
            "\nLargest N with {length}-byte payees: {maximum}. N={} fails: {}.\n",
            maximum + 1,
            next.expect_err("boundary")
        );
    }
    let mut mixed_maximum = 187;
    while mixed_profile(mixed_maximum + 1)
        .and_then(|manifest| {
            build_fixture(
                &manifest,
                (mixed_maximum + 1) as u64 * 20 * FIXTURE_COST,
                RootLock::P2sh20,
            )
        })
        .and_then(|split| verify_both(&split))
        .is_ok()
    {
        mixed_maximum += 1;
    }
    let mixed = build_fixture(
        &mixed_profile(mixed_maximum)?,
        mixed_maximum as u64 * 20 * FIXTURE_COST,
        RootLock::P2sh20,
    )?;
    let metrics = measure_input(&mixed.transaction, &mixed.source_outputs, 0, true)?;
    println!(
        "\nDensest mixed standard-script profile maximum N={mixed_maximum}: R={}, unlock={}, op={:.0}/{:.0}, hash={:.0}/{:.0}, tx/fee={}, fee/payee={:.3}, grossfront={}, netexternal={}. Next N fails {}.\n",
        mixed.redeem.len(),
        metrics.unlocking_length,
        metrics.operation_cost,
        metrics.operation_cost_limit,
        metrics.hash_iterations,
        metrics.hash_iteration_limit,
        mixed.size,
        mixed.fee as f64 / mixed_maximum as f64,
        mixed.gross_fronting,
        mixed.external_satoshis,
        mixed_profile(mixed_maximum + 1)
            .and_then(|manifest| build_fixture(
                &manifest,
                (mixed_maximum + 1) as u64 * 20 * FIXTURE_COST,
                RootLock::P2sh20
            ))
            .and_then(|split| verify_both(&split))
            .expect_err("mixed boundary")
    );
    println!(
        "Method: lengths from actual R and input unlocking; final libauth counters and limits plus full opcode-debug trace maxima via existing vm::measure_input; transaction bytes from libauth BCH serializer after fixed-length Schnorr funding signature; dust=3*(serialized output bytes+148) via existing output_dust. Gross fronting=sum token-output BCH minima+fee; net external subtracts inherited700, returned BCH change is excluded. The covenant minima conservatively reserve nine-byte token amounts (705 sats for P2SH32,675 P2PKH); BCH-only change dust546. 100000-byte standard transaction cap is checked by builder, 10000-byte unlock/redeem item caps checked before construction and by VMs. BCHN and market value are untested."
    );
    Ok(())
}
